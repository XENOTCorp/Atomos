//! Conservative HTTP cache eligibility and conditional response policy.
use crate::io::{Body, In, Method, Out};
use crate::status::Status;

pub(super) fn request_is_cacheable(req: &In<'_>) -> bool {
    matches!(req.method, Method::Get | Method::Head)
        && matches!(req.body, Body::Empty)
        // The cache key intentionally has no representation/auth dimensions.
        // Do not let these requests reuse a public, full-entity response.
        && ["range", "if-range", "authorization", "cookie", "cache-control"]
            .iter().all(|name| req.headers.get(name).is_none())
}

pub(super) fn response_is_cacheable(out: &Out) -> bool {
    !matches!(out.status.as_u16(), 206 | 304)
        && !out.headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("vary")
                || name.eq_ignore_ascii_case("set-cookie")
                || (name.eq_ignore_ascii_case("cache-control")
                    && value.split(',').any(|part| {
                        let directive = part.trim().split('=').next().unwrap_or("").trim();
                        ["private", "no-store", "no-cache"]
                            .iter()
                            .any(|name| directive.eq_ignore_ascii_case(name))
                    }))
        })
}

pub(super) fn not_modified(headers: &[(&str, &str)], cached: &Out) -> bool {
    if !(200..300).contains(&cached.status.as_u16()) {
        return false;
    }
    let header = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| *value)
    };
    let stored = |name: &str| {
        cached
            .headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_ref())
    };
    if let Some(value) = header("if-none-match") {
        // If-None-Match takes precedence even when it does not match.
        if value.trim() == "*" {
            return true;
        }
        return stored("etag").is_some_and(|etag| etag_list_matches(value, etag));
    }
    // Common unconditional hits do not scan response metadata at all.
    let Some(requested) = header("if-modified-since") else {
        return false;
    };
    stored("last-modified").is_some_and(|stored| requested.trim() == stored)
}

fn etag_list_matches(mut list: &str, stored: &str) -> bool {
    let stored = stored.strip_prefix("W/").unwrap_or(stored);
    while !list.trim().is_empty() {
        list = list.trim_start();
        let tag = list.strip_prefix("W/").unwrap_or(list);
        let Some(quoted) = tag.strip_prefix('"') else {
            return false;
        };
        let Some(end) = quoted.find('"') else {
            return false;
        };
        let candidate = &tag[..end + 2];
        let tail = tag[end + 2..].trim_start();
        if !tail.is_empty() && !tail.starts_with(',') {
            return false;
        }
        if candidate == stored {
            return true;
        }
        list = tail.strip_prefix(',').unwrap_or("");
    }
    false
}

pub(super) fn not_modified_response(cached: &Out) -> Out {
    let mut out = Out::empty(Status::NOT_MODIFIED);
    for (name, value) in &cached.headers {
        if [
            "etag",
            "last-modified",
            "cache-control",
            "expires",
            "vary",
            "content-location",
        ]
        .iter()
        .any(|key| name.eq_ignore_ascii_case(key))
        {
            out.headers.push((name.clone(), value.clone()));
        }
    }
    out
}
