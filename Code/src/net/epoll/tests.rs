use super::*;
use crate::atom::AtomCtx;
use crate::encode::encode_response;
use crate::error::ServeError;
use crate::route::Router;
use std::sync::Arc;

#[test]
fn epoll_run_is_blocking_signature() {
    let _: fn(Arc<Router>, Arc<AtomCtx>) -> Result<(), ServeError> = run;
}

#[test]
fn connection_id_tokens_never_collide_with_listener() {
    assert_ne!(ConnectionId::new(0, u32::MAX).as_u64(), TOKEN_LISTENER);
}

#[test]
fn recycled_slots_have_distinct_full_tokens() {
    let old = ConnectionId::new(1, 7).as_u64();
    let new = ConnectionId::new(2, 7).as_u64();
    assert_eq!(http_slot(old), http_slot(new));
    assert_ne!(old, new, "the server must reject stale full tokens");
}

#[test]
fn http_slot_is_connection_id_low_half() {
    assert_eq!(http_slot(ConnectionId::new(0, 7).as_u64()), Some(7));
    assert_eq!(http_slot(TOKEN_LISTENER), None);
    assert_eq!(
        http_slot(ConnectionId::new(0, CONN_CAP as u32).as_u64()),
        None
    );
}

#[test]
fn buf_capacity_covers_header_and_body() {
    assert_eq!(buf_capacity_for(16, 32), 48);
    assert_eq!(buf_capacity_for(usize::MAX, 1), usize::MAX);
}

#[test]
fn append_in_cap_does_not_grow() {
    let mut buf = Vec::with_capacity(16);
    assert!(append_in_cap(&mut buf, b"hello"));
    assert!(!append_in_cap(&mut buf, &[0; 32]));
    assert_eq!(buf.capacity(), 16);
    assert_eq!(buf, b"hello");
}

#[test]
fn bounded_growth_does_not_overshoot_the_wire_budget() {
    let mut buf = Vec::with_capacity(3);
    assert!(buffer::append_bounded(&mut buf, b"1234", 10));
    assert!(buffer::append_bounded(&mut buf, b"567", 10));
    assert!(buf.capacity() <= 10);
    assert!(!buffer::append_bounded(&mut buf, b"8901", 10));
    assert_eq!(buf, b"1234567");
}

#[test]
fn copy_into_out_does_not_grow() {
    let mut out = Vec::with_capacity(8);
    assert!(copy_into_out(&mut out, b"abcd"));
    assert!(!copy_into_out(&mut out, b"12345"));
    assert_eq!(out.capacity(), 8);
    assert_eq!(out, b"abcd");
}

#[test]
fn encode_scratch_fits_small_json_without_grow() {
    let out = crate::io::Out::json(
        crate::status::Status::OK,
        bytes::Bytes::from_static(br#"{"ok":true}"#),
    );
    let mut scratch = Vec::with_capacity(OUT_CAP);
    let cap = scratch.capacity();
    encode_response(&out, &mut scratch);
    assert_eq!(scratch.capacity(), cap);
    assert!(!scratch.is_empty());
    assert!(scratch.len() <= OUT_CAP);
}

#[test]
fn out_max_covers_byte_path_file() {
    assert_eq!(out_max(), SF_MIN as usize + OUT_CAP);
    assert!(out_max() > 64 * 1024);
}

#[test]
fn accept_reserve_is_4k() {
    assert_eq!(IN_CAP, 4096);
    const {
        assert!(IN_CAP < 16 * 1024);
    }
}
