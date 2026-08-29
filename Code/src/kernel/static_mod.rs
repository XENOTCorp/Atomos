//! Static files. Path traversal rejected. `/` → index.html.
//!
//! Files ≥ [`SF_MIN`] bytes are served as [`OutBody::File`]: the H1
//! epoll path sends them with `sendfile` (no userspace copy), and the
//! tokio paths materialize them into memory. A bounded LRU of open fds
//! is kept here (the open_file_cache equivalent) so repeated hits never
//! re-open/re-stat: the response cache never stores File bodies.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use bytes::Bytes;
use parking_lot::Mutex;

use crate::align::LineAtomicU64;
use crate::error::ServeError;
use crate::error_page::ErrorPage;
use crate::io::{CacheDirective, FileBody, In, Method, Out, OutBody};
use crate::mime;
use crate::module::Module;
use crate::status::Status;

/// Bodies at or above this size go through the sendfile path; smaller
/// ones stay on the wire-cache byte path (no syscall, no page-cache
/// dependency, headers+body already pre-encoded).
///
/// Measured on this box (loopback, stripped kernel whose loopback path
/// re-copies sendfile pages): byte path wins at 64 KiB (27.4k vs 16.5k
/// req/s), dead even at 128 KiB, sendfile wins at 256 KiB (2.09 vs 1.59
/// GB/s). On a real NIC sendfile wins from far smaller sizes (no
/// loopback re-copy): lower this for NIC deployments. Override with
/// `ATOMOS_SF_MIN` (bytes) for A/B measurement.
pub const SF_MIN: u64 = 128 * 1024;

fn sf_min() -> u64 {
    static SF: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *SF.get_or_init(|| {
        std::env::var("ATOMOS_SF_MIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(SF_MIN)
    })
}

/// Open-file LRU cap: at most this many fds are ever held by the static
/// module (shared across workers), far below any sane rlimit.
const FD_CACHE_MAX: usize = 64;

struct FdEntry {
    file: Arc<std::fs::File>,
    len: u64,
    /// Recency stamp for LRU eviction (monotonic counter).
    last: u64,
}

struct FdCache {
    map: HashMap<PathBuf, FdEntry>,
    seq: u64,
}

impl FdCache {
    fn bump(&mut self) -> u64 {
        self.seq = self.seq.wrapping_add(1);
        self.seq
    }

    /// Drop the stalest entry once the cap is exceeded (called after
    /// every insert, so at most one entry over the cap exists).
    fn evict_if_over(&mut self) {
        if self.map.len() > FD_CACHE_MAX {
            if let Some((p, _)) = self.map.iter().min_by_key(|(_, e)| e.last) {
                let stale = p.clone();
                self.map.remove(&stale);
            }
        }
    }
}

pub struct StaticMod {
    root: PathBuf,
    errors: ErrorPage,
    pub hits: LineAtomicU64,
    fd: Mutex<FdCache>,
}

impl StaticMod {
    pub fn new(root: PathBuf, errors: ErrorPage) -> Arc<Self> {
        Arc::new(Self {
            root,
            errors,
            hits: LineAtomicU64::new(0),
            fd: Mutex::new(FdCache {
                map: HashMap::with_capacity(FD_CACHE_MAX),
                seq: 0,
            }),
        })
    }

    fn not_found(&self) -> Out {
        let body = self.errors.render(Status::NOT_FOUND, "not found");
        Out {
            status: Status::NOT_FOUND,
            reason: None,
            headers: vec![("Content-Type".into(), "text/html; charset=utf-8".into())],
            body: OutBody::Raw(body),
            cache: CacheDirective::No,
            flags: crate::flags::FlagSet::empty(),
        }
    }
}

impl Module for StaticMod {
    fn name(&self) -> &'static str {
        "static"
    }

    fn handle(&self, req: &In<'_>) -> Result<Out, ServeError> {
        self.hits.v.fetch_add(1, Ordering::Relaxed);
        if req.method != Method::Get && req.method != Method::Head {
            return Err(ServeError::Parse);
        }
        let rel = if req.path == "/" {
            "index.html"
        } else {
            req.path.trim_start_matches('/')
        };
        let dest = match safe_join(&self.root, rel) {
            Some(p) => p,
            None => return Ok(self.not_found()),
        };
        let ct = mime::from_path(dest.to_str().unwrap_or(rel));
        let ttl = if ct.starts_with("text/html") {
            5_000
        } else {
            60_000
        };
        // Large-file hit path: the fd LRU serves the body with no
        // stat/open syscalls at all (the byte path's response-cache
        // equivalent for the sendfile path).
        let mut fd = self.fd.lock();
        let stamp = fd.seq.wrapping_add(1);
        fd.seq = stamp;
        if let Some(e) = fd.map.get_mut(&dest) {
            e.last = stamp;
            let file = e.file.clone();
            let len = e.len;
            drop(fd);
            return Ok(ranged_file(req, ct, ttl, file, len));
        }
        drop(fd);
        // Miss path: extension fallback + one metadata call, then either
        // the sendfile fd (inserted into the LRU) or the byte read.
        let dest = if !dest.exists() && dest.extension().is_none() {
            dest.with_extension("html")
        } else {
            dest
        };
        let ct = mime::from_path(dest.to_str().unwrap_or(rel));
        let ttl = if ct.starts_with("text/html") {
            5_000
        } else {
            60_000
        };
        let meta = match std::fs::metadata(&dest) {
            Ok(m) if m.is_file() => m,
            _ => return Ok(self.not_found()),
        };
        let len = meta.len();
        if len >= sf_min() {
            let mut fd = self.fd.lock();
            let f = match std::fs::File::open(&dest) {
                Ok(f) => f,
                Err(_) => return Ok(self.not_found()),
            };
            let last = fd.bump();
            fd.map.insert(
                dest.clone(),
                FdEntry {
                    file: Arc::new(f),
                    len,
                    last,
                },
            );
            fd.evict_if_over();
            let e = fd.map.get(&dest).expect("just inserted");
            let file = e.file.clone();
            drop(fd);
            return Ok(ranged_file(req, ct, ttl, file, len));
        }
        match std::fs::read(&dest) {
            Ok(b) => Ok(ranged_bytes(req, ct, ttl, Bytes::from(b))),
            Err(_) => Ok(self.not_found()),
        }
    }
}

/// `Ok(None)` = whole entity. `Ok(Some((off,n)))` = one range. `Err` = 416.
fn parse_byte_range(h: Option<&str>, len: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(h) = h else {
        return Ok(None);
    };
    let rest = h.strip_prefix("bytes=").ok_or(())?;
    if rest.contains(',') {
        return Err(());
    }
    if let Some(suf) = rest.strip_prefix('-') {
        let n: u64 = suf.parse().map_err(|_| ())?;
        if n == 0 || len == 0 {
            return Err(());
        }
        let n = n.min(len);
        return Ok(Some((len - n, n)));
    }
    let (a, b) = rest.split_once('-').ok_or(())?;
    let start: u64 = a.parse().map_err(|_| ())?;
    if start >= len {
        return Err(());
    }
    let end = if b.is_empty() {
        len - 1
    } else {
        b.parse::<u64>().map_err(|_| ())?
    };
    if end < start {
        return Err(());
    }
    let end = end.min(len - 1);
    Ok(Some((start, end - start + 1)))
}

fn ranged_file(req: &In<'_>, ct: &str, ttl: u32, file: Arc<std::fs::File>, len: u64) -> Out {
    match parse_byte_range(req.headers.get("range"), len) {
        Ok(None) => static_out(
            Status::OK,
            ct,
            ttl,
            len,
            vec![],
            OutBody::File(FileBody {
                file,
                offset: 0,
                len,
            }),
        ),
        Ok(Some((off, n))) => static_out(
            Status::PARTIAL_CONTENT,
            ct,
            ttl,
            len,
            vec![(
                "Content-Range".into(),
                format!("bytes {off}-{}/{len}", off + n - 1).into(),
            )],
            OutBody::File(FileBody {
                file,
                offset: off,
                len: n,
            }),
        ),
        Err(()) => range_not_satisfiable(ct, len),
    }
}

fn ranged_bytes(req: &In<'_>, ct: &str, ttl: u32, b: Bytes) -> Out {
    let len = b.len() as u64;
    match parse_byte_range(req.headers.get("range"), len) {
        Ok(None) => static_out(Status::OK, ct, ttl, len, vec![], OutBody::Raw(b)),
        Ok(Some((off, n))) => {
            let s = off as usize;
            let e = s + n as usize;
            static_out(
                Status::PARTIAL_CONTENT,
                ct,
                ttl,
                len,
                vec![(
                    "Content-Range".into(),
                    format!("bytes {off}-{}/{len}", off + n - 1).into(),
                )],
                OutBody::Raw(b.slice(s..e)),
            )
        }
        Err(()) => range_not_satisfiable(ct, len),
    }
}

fn static_out(
    status: Status,
    ct: &str,
    ttl: u32,
    entity_len: u64,
    extra: Vec<(Box<str>, Box<str>)>,
    body: OutBody,
) -> Out {
    let mut headers = vec![
        ("Content-Type".into(), ct.into()),
        ("Accept-Ranges".into(), "bytes".into()),
        ("ETag".into(), format!("\"{entity_len}\"").into()),
    ];
    headers.extend(extra);
    Out {
        status,
        reason: None,
        headers,
        body,
        cache: CacheDirective::Global { ttl_ms: ttl },
        flags: crate::flags::FlagSet::empty(),
    }
}

fn range_not_satisfiable(ct: &str, len: u64) -> Out {
    Out {
        status: Status::RANGE_NOT_SATISFIABLE,
        reason: None,
        headers: vec![
            ("Content-Type".into(), ct.into()),
            ("Content-Range".into(), format!("bytes */{len}").into()),
        ],
        body: OutBody::Empty,
        cache: CacheDirective::No,
        flags: crate::flags::FlagSet::empty(),
    }
}

fn safe_join(root: &Path, rel: &str) -> Option<PathBuf> {
    if rel.contains('\0') {
        return None;
    }
    let mut dest = PathBuf::from(root);
    for c in Path::new(rel).components() {
        match c {
            Component::Normal(s) => dest.push(s),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flags::FlagSet;
    use crate::io::HeaderView;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn peer() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1)
    }

    #[test]
    fn serves_index_and_txt_and_404() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), b"<h1>ok</h1>").unwrap();
        std::fs::write(dir.path().join("x.txt"), b"hello").unwrap();
        let m = StaticMod::new(dir.path().to_path_buf(), ErrorPage::builtin());
        let dummy_headers = HeaderView { pairs: vec![] };
        let mk = |path: &'static str| In {
            method: Method::Get,
            path,
            query: "",
            headers: HeaderView {
                pairs: dummy_headers.pairs.clone(),
            },
            body: crate::io::Body::Empty,
            peer: peer(),
            flags: FlagSet::empty(),
        };
        assert_eq!(std::mem::align_of::<LineAtomicU64>(), 64);
        assert_eq!(std::mem::size_of::<LineAtomicU64>(), 64);
        let a = m.handle(&mk("/")).unwrap();
        assert_eq!(a.status.as_u16(), 200);
        assert!(std::str::from_utf8(a.body.as_bytes()).unwrap().contains("<h1>ok"));
        let b = m.handle(&mk("/x.txt")).unwrap();
        assert_eq!(b.status.as_u16(), 200);
        assert_eq!(b.body.as_bytes(), b"hello");
        let c = m.handle(&mk("/no")).unwrap();
        assert_eq!(c.status.as_u16(), 404);
        let html = std::str::from_utf8(c.body.as_bytes()).unwrap();
        assert!(html.contains("404"));
    }

    #[test]
    fn big_file_served_as_file_body_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        // Above SF_MIN so the sendfile path is taken.
        let blob: Vec<u8> = (0..(SF_MIN as usize + 4096) as u32)
            .map(|i| (i % 251) as u8)
            .collect();
        std::fs::write(dir.path().join("big.bin"), &blob).unwrap();
        let m = StaticMod::new(dir.path().to_path_buf(), ErrorPage::builtin());
        let dummy_headers = HeaderView { pairs: vec![] };
        let req = In {
            method: Method::Get,
            path: "/big.bin",
            query: "",
            headers: HeaderView {
                pairs: dummy_headers.pairs.clone(),
            },
            body: crate::io::Body::Empty,
            peer: peer(),
            flags: FlagSet::empty(),
        };
        let out = m.handle(&req).unwrap();
        assert_eq!(out.status.as_u16(), 200);
        match &out.body {
            OutBody::File(f) => {
                assert_eq!(f.len as usize, blob.len());
                // The fd-cache hit path returns the same range with no
                // re-stat; read it back and compare.
                let back = f.read_to_bytes().unwrap();
                assert_eq!(back.as_ref(), blob.as_slice());
            }
            other => panic!("expected OutBody::File, got {other:?}"),
        }
    }

    #[test]
    fn fd_cache_stays_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let m = StaticMod::new(dir.path().to_path_buf(), ErrorPage::builtin());
        let dummy_headers = HeaderView { pairs: vec![] };
        for i in 0..(FD_CACHE_MAX * 2) {
            let name = format!("f{i}.bin");
            std::fs::write(dir.path().join(&name), vec![7u8; SF_MIN as usize]).unwrap();
            let path = format!("/{name}");
            let req = In {
                method: Method::Get,
                path: &path,
                query: "",
                headers: HeaderView {
                    pairs: dummy_headers.pairs.clone(),
                },
                body: crate::io::Body::Empty,
                peer: peer(),
                flags: FlagSet::empty(),
            };
            assert_eq!(m.handle(&req).unwrap().status.as_u16(), 200);
        }
        assert!(
            m.fd.lock().map.len() <= FD_CACHE_MAX,
            "fd LRU exceeded its cap: {}",
            m.fd.lock().map.len()
        );
    }
}
