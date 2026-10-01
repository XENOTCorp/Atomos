//! HTTP parsing and response dispatch, independent of readiness registration.
use super::buffer::append_bounded;
use super::conn::{Conn, PendingSf};
use super::queue::{finish_response, queue_bytes};
use super::write::flush_out;
use super::{buf_capacity_for, tlsio, COMPACT_THRESHOLD};
use crate::access_log;
use crate::cache::CachedResponse;
use crate::encode::{append_chunk, append_chunk_end, encode_head, encode_response};
use crate::flags::FlagSet;
use crate::io::{Body, HeaderView, In, Out, OutBody};
use crate::parse::{
    decode_chunked_into, looks_like_json, parse_request_with_limits, scan_json, ParseStatus,
};
use crate::route::Router;
use crate::status::Status;
use fds::util::now_ticks;
use std::io;
use std::time::Instant;

/// Drain the previous response before dispatching another request. Buffered
/// pipelined requests must resume here: edge-triggered epoll may never send
/// another readable event for bytes we already took out of the socket.
pub(super) fn drive_connection(
    c: &mut Conn<'_>,
    router: &Router,
    enc: &mut Vec<u8>,
    readable: bool,
    writable: bool,
) -> io::Result<bool> {
    let was_pending = c.has_pending_output();
    if was_pending && (readable || writable) {
        flush_out(c)?;
    }
    if c.has_pending_output() {
        return Ok(true);
    }
    if c.close_after_write {
        return Ok(false);
    }
    // Readable edges may have arrived while writes were blocked. Drain the
    // socket after resumption even if no request bytes are buffered yet.
    if readable || was_pending || c.buf.len() > c.pos {
        return read_and_serve(c, router, enc);
    }
    Ok(true)
}

fn read_and_serve(c: &mut Conn<'_>, router: &Router, enc: &mut Vec<u8>) -> io::Result<bool> {
    let mut tmp = [0u8; 4096];
    let mut eof = false;
    loop {
        let n = if c.tls.is_none() {
            match c.stream.read(&mut tmp) {
                Ok(0) => {
                    eof = true;
                    break;
                }
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => return Ok(false),
            }
        } else {
            match tlsio::read_plain(c, &mut tmp) {
                Ok(None) => break,
                Ok(Some(0)) => {
                    if c.tls.as_ref().is_some_and(|t| t.is_handshaking()) {
                        return Ok(true);
                    }
                    eof = true;
                    break;
                }
                Ok(Some(n)) => n,
                Err(_) => return Ok(false),
            }
        };
        c.last_rw = Instant::now();
        if c.hdr_t0.is_none() {
            c.hdr_t0 = Some(c.last_rw);
        }
        let max = buf_capacity_for(router.cfg.max_header_bytes, router.cfg.max_body_bytes);
        // Reclaim consumed space before growing: ordinary keep-alive traffic
        // should stay in the accept-time buffer instead of accumulating 64 KiB.
        if c.pos > 0 && c.buf.len().saturating_add(n) > c.buf.capacity() {
            c.buf.copy_within(c.pos.., 0);
            c.buf.truncate(c.buf.len() - c.pos);
            c.pos = 0;
        }
        if !append_bounded(&mut c.buf, &tmp[..n], max) {
            return Ok(false);
        }
        // Hot state: sequence + activity on every step (the
        // FDS hot/cold split in action).
        let hot = &mut c.slot.conn_mut().hot;
        hot.seq = hot.seq.wrapping_add(n as u32);
        hot.last_activity = now_ticks();
    }
    loop {
        // Compact consumed bytes once per 64 KiB (amortized memmove).
        if c.pos >= COMPACT_THRESHOLD {
            c.buf.drain(..c.pos);
            c.pos = 0;
        }
        match parse_request_with_limits(
            &c.buf[c.pos..],
            router.cfg.max_header_bytes,
            router.cfg.max_body_bytes,
        ) {
            Ok(ParseStatus::Partial) => return Ok(!eof),
            Err(error) => {
                reply_status(c, enc, Status::from_u16(error.status()))?;
                return Ok(false);
            }
            Ok(ParseStatus::Complete(p)) => {
                if p.content_length > router.cfg.max_body_bytes {
                    reply_status(c, enc, Status::from_u16(413))?;
                    return Ok(false);
                }
                let need_rel = p.wire_end;
                if c.buf.len() - c.pos < need_rel {
                    return Ok(true);
                }
                let need = c.pos + need_rel;
                if c.buf.get(c.pos) == Some(&b'P') && c.buf[c.pos..].starts_with(b"PRI ") {
                    return Ok(false);
                }
                if p.upgrade {
                    reply_status(c, enc, Status::UPGRADE_REQUIRED)?;
                    return Ok(false);
                }
                let head = p.method == crate::io::Method::Head;
                let mut decoded = Vec::new();
                let body_bytes: &[u8] = if p.chunked {
                    let start = c.pos + p.header_end;
                    decode_chunked_into(&c.buf[start..need], &mut decoded)
                        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "chunked"))?;
                    &decoded
                } else {
                    &c.buf[c.pos + p.header_end..need]
                };
                if looks_like_json(body_bytes)
                    && scan_json(body_bytes, router.cfg.max_json_depth).is_err()
                {
                    reply_status(c, enc, Status::BAD_REQUEST)?;
                    return Ok(false);
                }
                let body = if body_bytes.is_empty() {
                    Body::Empty
                } else if looks_like_json(body_bytes) {
                    Body::Json(body_bytes)
                } else {
                    Body::Raw(body_bytes)
                };
                let req = In {
                    method: p.method,
                    path: p.path,
                    query: p.query,
                    headers: HeaderView { pairs: p.headers },
                    body,
                    peer: c.peer,
                    flags: FlagSet::empty(),
                };
                let ka = p.keepalive;
                if let Some(cached) = router.cached_h1(&req) {
                    if router.cfg.access_log {
                        let (status, len) = match &cached {
                            CachedResponse::Wire {
                                status, body_len, ..
                            } => (*status, *body_len),
                            CachedResponse::Response(out) => (out.status, out.body.len()),
                        };
                        access_log::emit(req.method, req.path, status.as_u16(), len);
                    }
                    drop(req);
                    c.pos = need;
                    c.served = true;
                    c.hdr_t0 = None;
                    match cached {
                        CachedResponse::Wire { bytes, .. } => queue_bytes(c, &bytes)?,
                        CachedResponse::Response(out) => {
                            if head {
                                encode_head(&out, enc);
                            } else {
                                encode_response(&out, enc);
                            }
                            queue_bytes(c, enc)?;
                        }
                    }
                    if finish_response(c, ka)? {
                        return Ok(c.has_pending_output());
                    }
                    continue;
                }
                let t0 = Instant::now();
                let mut out = router.dispatch(req);
                if t0.elapsed().as_millis() as u64 > router.cfg.module_timeout_ms.max(1) {
                    out = Out::empty(Status::GATEWAY_TIMEOUT);
                }
                if router.cfg.access_log {
                    access_log::emit(p.method, p.path, out.status.as_u16(), out.body.len());
                }
                enc.clear();
                if head || !out.status.allows_body() {
                    encode_head(&out, enc);
                    queue_bytes(c, enc)?;
                } else if matches!(out.body, OutBody::Stream(_)) {
                    encode_response(&out, enc);
                    queue_bytes(c, enc)?;
                    if let OutBody::Stream(s) = &out.body {
                        let mut rx = s.take();
                        while let Ok(chunk) = rx.try_recv() {
                            enc.clear();
                            append_chunk(enc, &chunk);
                            queue_bytes(c, enc)?;
                        }
                    }
                    enc.clear();
                    append_chunk_end(enc);
                    queue_bytes(c, enc)?;
                } else {
                    encode_response(&out, enc);
                    queue_bytes(c, enc)?;
                    if let OutBody::File(f) = &out.body {
                        c.pending_sf = Some(PendingSf {
                            file: f.file.clone(),
                            offset: f.offset as libc::off_t,
                            len: f.len,
                        });
                    }
                }
                c.pos = need;
                c.served = true;
                c.hdr_t0 = None;
                if finish_response(c, ka)? {
                    return Ok(c.has_pending_output());
                }
            }
        }
    }
}

pub(super) fn reply_status(c: &mut Conn<'_>, enc: &mut Vec<u8>, st: Status) -> io::Result<()> {
    let out = Out::empty(st);
    enc.clear();
    encode_response(&out, enc);
    queue_bytes(c, enc)?;
    let _ = flush_out(c);
    Ok(())
}
