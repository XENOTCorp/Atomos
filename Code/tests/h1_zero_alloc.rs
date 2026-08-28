//! TEST-04 / ALLOC-01: the declared H1 hot path allocates nothing
//! after warm-up.
//!
//! The situation is the epoll HTTP/1.1 receive/encode/cache-hit loop.
//! Working storage is pre-sized on the control path (accept-time request
//! buffer, per-worker encode scratch, per-connection `out`). This binary
//! counts heap allocations on the calling thread and asserts the hot
//! operations themselves perform zero.
//!
//! In scope: `encode_response` into a reserved scratch, `append_in_cap` /
//! `copy_into_out`, `ResponseCache::get_wire` after the first put.
//! Out of scope: `parse_request` (still builds a header `Vec`).

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use bytes::Bytes;

use atomos::cache::ResponseCache;
use atomos::encode::encode_response;
use atomos::epoll::{append_in_cap, buf_capacity_for, copy_into_out, OUT_CAP};
use atomos::flags::FlagSet;
use atomos::io::{CacheDirective, Method, Out, OutBody};
use atomos::status::Status;

thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

struct CountingAlloc;

// SAFETY: every operation is forwarded verbatim to `System`; the
// counter increment happens before the forward and neither reads nor
// writes the returned pointer.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCS.with(|c| c.set(c.get() + 1));
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.with(|c| c.set(c.get() + 1));
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

fn reset() {
    ALLOCS.with(|c| c.set(0));
}

fn count() -> usize {
    ALLOCS.with(|c| c.get())
}

fn sample() -> Out {
    Out {
        status: Status::OK,
        reason: None,
        headers: vec![("Content-Type".into(), "application/json".into())],
        body: OutBody::Json(Bytes::from_static(br#"{"ok":true}"#)),
        cache: CacheDirective::Global { ttl_ms: 60_000 },
        flags: FlagSet::empty(),
    }
}

#[test]
fn h1_hot_path_allocates_zero_after_warmup() {
    let out = sample();

    let buf_cap = buf_capacity_for(8192, 1_048_576);
    let mut buf = Vec::with_capacity(buf_cap);
    let mut scratch = Vec::with_capacity(OUT_CAP);
    let mut queued = Vec::with_capacity(OUT_CAP);

    encode_response(&out, &mut scratch);
    assert!(
        scratch.len() <= OUT_CAP,
        "sample response must fit the encode scratch"
    );
    assert!(scratch.capacity() >= OUT_CAP);

    let cache = ResponseCache::new(16, 1 << 20);
    cache.put(Method::Get, "/health", "", &out);
    let warm = cache
        .get_wire(Method::Get, "/health", "")
        .expect("warm wire");
    assert!(!warm.is_empty());

    assert!(append_in_cap(&mut buf, b"GET /health HTTP/1.1\r\n\r\n"));
    assert!(copy_into_out(&mut queued, scratch.as_slice()));

    let cap_buf = buf.capacity();
    let cap_scratch = scratch.capacity();
    let cap_queued = queued.capacity();

    reset();
    for _ in 0..10_000 {
        encode_response(&out, &mut scratch);
        buf.clear();
        assert!(append_in_cap(&mut buf, b"GET /health HTTP/1.1\r\n\r\n"));
        queued.clear();
        assert!(copy_into_out(&mut queued, scratch.as_slice()));
        let wire = cache
            .get_wire(Method::Get, "/health", "")
            .expect("wire hit");
        assert_eq!(wire.as_ref().as_ref(), scratch.as_slice());
    }

    assert_eq!(
        count(),
        0,
        "declared H1 hot path allocated (ALLOC-01 / TEST-04)"
    );
    assert_eq!(buf.capacity(), cap_buf);
    assert_eq!(scratch.capacity(), cap_scratch);
    assert_eq!(queued.capacity(), cap_queued);
}
