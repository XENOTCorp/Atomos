use super::*;
use crate::flags::FlagSet;
use crate::io::{Body, HeaderView};

fn sample() -> Out {
    let mut out = Out::json(Status::OK, Bytes::from_static(br#"{"ok":true}"#));
    out.cache = CacheDirective::Global { ttl_ms: 60_000 };
    out
}

fn named(id: &str) -> Out {
    let mut out = sample();
    out.cache = CacheDirective::Named {
        ruleset: id.into(),
        ttl_ms: 60_000,
    };
    out
}

fn request<'a>(method: Method, headers: Vec<(&'a str, &'a str)>) -> In<'a> {
    In {
        method,
        path: "/x",
        query: "",
        headers: HeaderView { pairs: headers },
        body: Body::Empty,
        peer: "127.0.0.1:1".parse().unwrap(),
        flags: FlagSet::empty(),
    }
}

#[test]
fn same_thread_put_get() {
    let cache = ResponseCache::new(16, 1 << 20);
    cache.put(Method::Get, "/x", "", &sample());
    assert!(cache.get(Method::Get, "/x", "").is_some());
    assert!(cache.get_wire(Method::Get, "/x", "").is_some());
}

#[test]
fn distinct_paths_are_not_aliases() {
    let cache = ResponseCache::new(16, 1 << 20);
    cache.put(Method::Get, "/a", "", &sample());
    cache.put(Method::Get, "/b", "", &sample());
    assert!(cache.get(Method::Get, "/a", "").is_some());
    assert!(cache.get(Method::Get, "/b", "").is_some());
    assert!(cache.get(Method::Get, "/c", "").is_none());
}

#[test]
fn independent_caches_are_isolated_even_when_alternating() {
    let first = ResponseCache::new(16, 1 << 20);
    let second = ResponseCache::new(16, 1 << 20);
    first.put(Method::Get, "/x", "", &sample());
    assert!(second.get(Method::Get, "/x", "").is_none());
    let mut different = sample();
    different.body = OutBody::Raw(Bytes::from_static(b"other router"));
    second.put(Method::Get, "/x", "", &different);
    for _ in 0..10 {
        assert_eq!(
            first.get(Method::Get, "/x", "").unwrap().body.as_bytes(),
            br#"{"ok":true}"#
        );
        assert_eq!(
            second.get(Method::Get, "/x", "").unwrap().body.as_bytes(),
            b"other router"
        );
    }
    first.invalidate();
    assert!(first.get(Method::Get, "/x", "").is_none());
    assert!(second.get(Method::Get, "/x", "").is_some());
}

#[test]
fn clones_share_entries_and_epochs() {
    let cache = ResponseCache::new(16, 1 << 20);
    let clone = cache.clone();
    cache.put(Method::Get, "/x", "", &sample());
    assert!(clone.get(Method::Get, "/x", "").is_some());
    clone.invalidate();
    assert!(cache.get(Method::Get, "/x", "").is_none());
}

#[test]
fn invalidate_drops_global_not_named() {
    let cache = ResponseCache::new(16, 1 << 20);
    cache.put(Method::Get, "/g", "", &sample());
    cache.put(Method::Get, "/n", "", &named("notes"));
    cache.invalidate();
    assert!(cache.get_wire(Method::Get, "/g", "").is_none());
    assert!(cache.get_wire(Method::Get, "/n", "").is_some());
}

#[test]
fn invalidate_named_drops_only_that_name() {
    let cache = ResponseCache::new(16, 1 << 20);
    cache.put(Method::Get, "/g", "", &sample());
    cache.put(Method::Get, "/n", "", &named("notes"));
    cache.invalidate_named("notes");
    assert!(cache.get_wire(Method::Get, "/g", "").is_some());
    assert!(cache.get_wire(Method::Get, "/n", "").is_none());
}

#[test]
fn all_invalidation_rejects_late_work_from_an_old_generation() {
    let cache = ResponseCache::new(16, 1 << 20);
    let stamp = cache.stamp();
    cache.put(Method::Get, "/g", "", &sample());
    cache.put(Method::Get, "/n", "", &named("notes"));
    cache.invalidate_all();
    assert!(cache.get(Method::Get, "/g", "").is_none());
    assert!(cache.get(Method::Get, "/n", "").is_none());
    cache.put_stamped(Method::Get, "/late", "", &named("notes"), &stamp);
    assert!(cache.get(Method::Get, "/late", "").is_none());
    cache.put(Method::Get, "/fresh", "", &named("notes"));
    assert!(cache.get(Method::Get, "/fresh", "").is_some());
}

#[test]
fn late_work_cannot_repopulate_a_purged_named_or_global_epoch() {
    let cache = ResponseCache::new(16, 1 << 20);
    let stamp = cache.stamp();
    cache.invalidate();
    cache.invalidate_named("notes");
    cache.put_stamped(Method::Get, "/g", "", &sample(), &stamp);
    cache.put_stamped(Method::Get, "/n", "", &named("notes"), &stamp);
    assert!(cache.get(Method::Get, "/g", "").is_none());
    assert!(cache.get(Method::Get, "/n", "").is_none());
}

#[test]
fn epoch_is_one_cache_line() {
    assert_eq!(std::mem::align_of::<LineAtomicU64>(), 64);
    assert_eq!(std::mem::size_of::<LineAtomicU64>(), 64);
}

#[test]
fn other_thread_does_not_see_put() {
    let cache = ResponseCache::new(16, 1 << 20);
    cache.put(Method::Get, "/x", "", &sample());
    std::thread::scope(|scope| {
        scope.spawn(|| assert!(cache.get(Method::Get, "/x", "").is_none()));
    });
}

#[test]
fn repeated_replacements_do_not_leak_budget_or_fifo_keys() {
    let cache = ResponseCache::new(4, 1024);
    for _ in 0..1000 {
        cache.put(Method::Get, "/x", "", &sample());
    }
    cache.with_inner(|inner| {
        let (entries, fifo, bytes) = inner.usage();
        assert_eq!((entries, fifo), (1, 1));
        assert!(bytes <= 1024);
    });
    cache.put(Method::Get, "/y", "", &sample());
    assert!(cache.get(Method::Get, "/x", "").is_some());
    assert!(cache.get(Method::Get, "/y", "").is_some());
}

#[test]
fn oversized_entries_do_not_evict_smaller_ones() {
    let cache = ResponseCache::new(4, 1024);
    cache.put(Method::Get, "/x", "", &sample());
    let mut large = sample();
    large.body = OutBody::Raw(Bytes::from(vec![0; 2048]));
    cache.put(Method::Get, "/large", "", &large);
    assert!(cache.get(Method::Get, "/large", "").is_none());
    assert!(cache.get(Method::Get, "/x", "").is_some());
}

#[test]
fn count_and_byte_caps_hold_during_churn() {
    let cache = ResponseCache::new(3, 1024);
    for i in 0..100 {
        cache.put(Method::Get, &format!("/{i}"), "", &sample());
        cache.with_inner(|inner| {
            let (entries, fifo, bytes) = inner.usage();
            assert!(entries <= 3 && fifo == entries && bytes <= 1024);
        });
    }
}

#[test]
fn request_and_response_variants_do_not_pollute_public_cache() {
    let cache = ResponseCache::new(16, 1 << 20);
    cache.put(Method::Get, "/x", "", &sample());
    for header in [
        "Range",
        "If-Range",
        "Authorization",
        "Cookie",
        "Cache-Control",
    ] {
        assert!(cache
            .get_h1(&request(Method::Get, vec![(header, "x")]))
            .is_none());
        assert!(cache
            .get_for(&request(Method::Get, vec![(header, "x")]))
            .is_none());
    }
    let mut req = request(Method::Get, vec![]);
    req.body = Body::Raw(b"body");
    assert!(cache.get_for(&req).is_none());
    for status in [Status::PARTIAL_CONTENT, Status::NOT_MODIFIED] {
        let mut out = sample();
        out.status = status;
        cache.put(Method::Get, "/uncached", "", &out);
        assert!(cache.get(Method::Get, "/uncached", "").is_none());
    }
    for (name, value) in [
        ("Vary", "Accept"),
        ("Set-Cookie", "a=b"),
        ("Cache-Control", "private"),
        ("Cache-Control", "no-store"),
    ] {
        let mut out = sample();
        out.headers.push((name.into(), value.into()));
        cache.put(Method::Get, "/uncached", "", &out);
        assert!(cache.get(Method::Get, "/uncached", "").is_none());
    }
}

#[test]
fn conditional_precedence_weak_tags_lists_and_validator_headers() {
    let cache = ResponseCache::new(4, 1024);
    let mut out = sample();
    out.headers.extend([
        ("ETag".into(), "\"a,b\"".into()),
        ("Last-Modified".into(), "date".into()),
    ]);
    cache.put(Method::Get, "/x", "", &out);
    assert!(!ResponseCache::not_modified(
        &[
            ("If-None-Match", "\"wrong\""),
            ("If-Modified-Since", "date")
        ],
        &out
    ));
    for value in ["W/\"a,b\"", "\"other\", W/\"a,b\"", "*"] {
        let req = request(Method::Get, vec![("If-None-Match", value)]);
        let hit = cache.get_for(&req).unwrap();
        assert_eq!(hit.status, Status::NOT_MODIFIED);
        assert!(hit
            .headers
            .iter()
            .any(|(name, value)| name.as_ref() == "ETag" && value.as_ref() == "\"a,b\""));
    }
}

#[test]
fn head_wire_never_contains_the_body() {
    let cache = ResponseCache::new(4, 1024);
    cache.put(Method::Head, "/x", "", &sample());
    let CachedResponse::Wire {
        bytes, body_len, ..
    } = cache.get_h1(&request(Method::Head, vec![])).unwrap()
    else {
        panic!("wire expected")
    };
    assert_eq!(body_len, 0);
    assert!(bytes.ends_with(b"\r\n\r\n"));
    assert!(String::from_utf8_lossy(&bytes).contains("Content-Length: 11"));
}
