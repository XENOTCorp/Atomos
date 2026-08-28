//! Keys list and token helpers.
use std::io::Read;
use std::path::Path;
use serde_json::{json, Value};
use super::io_error_json;

pub fn random_token() -> String {
    let mut b = [0u8; 12];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut b);
    }
    let mut out = String::with_capacity(24);
    for x in b {
        let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{x:02x}"));
    }
    out
}

pub(crate) fn mask(s: &str) -> String {
    if s.len() <= 4 {
        "****".into()
    } else {
        format!("{}…{}", &s[..2], &s[s.len() - 2..])
    }
}
pub(crate) fn keys_list(path: &Path, reveal: bool) -> Value {
    if !path.exists() {
        return json!({"ok": true, "keys": []});
    }
    match std::fs::read(path) {
        Err(e) => io_error_json(&e, path),
        Ok(raw) => match serde_json::from_slice::<Value>(&raw) {
            Err(e) => json!({"ok": false, "error": "json", "message": e.to_string()}),
            Ok(doc) => {
                let keys = match doc.get("keys").and_then(|k| k.as_array()) {
                    Some(arr) => arr
                        .iter()
                        .enumerate()
                        .map(|(i, v)| {
                            if let Some(t) = v.get("token").and_then(|x| x.as_str()) {
                                let shown = if reveal {
                                    t.to_string()
                                } else {
                                    mask(t)
                                };
                                json!({
                                    "index": i,
                                    "token": shown,
                                    "note": v.get("note").and_then(|x| x.as_str()).unwrap_or(""),
                                    "created_s": v.get("created_s").and_then(|x| x.as_u64()).unwrap_or(0)
                                })
                            } else {
                                json!({"index": i, "value": v})
                            }
                        })
                        .collect::<Vec<_>>(),
                    None => Vec::new(),
                };
                json!({"ok": true, "keys": keys})
            }
        },
    }
}
