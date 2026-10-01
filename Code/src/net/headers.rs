//! Shared outbound header validation. Modules cannot supply framing fields
//! or inject a second response through header names, values or reason phrases.

pub(crate) fn transport_owned(name: &str) -> bool {
    [
        "content-length",
        "transfer-encoding",
        "connection",
        "keep-alive",
        "proxy-connection",
        "upgrade",
        "trailer",
    ]
    .iter()
    .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

pub(crate) fn valid_value(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte == b'\t' || (byte >= 32 && byte != 127))
}
