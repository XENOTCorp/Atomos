//! JSON response bytes. Domain: serde-serializable module bodies.
//! Thread-local `to_writer` + copy-out beats `serde_json::to_vec` then `Bytes`
//! on typical API payloads. Nested borrow falls back to `to_vec`.

use std::cell::RefCell;

use bytes::Bytes;
use serde::Serialize;

thread_local! {
    static BUF: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(4096));
}

pub fn to_bytes<T: Serialize>(v: &T) -> Bytes {
    BUF.with(|cell| match cell.try_borrow_mut() {
        Ok(mut buf) => {
            buf.clear();
            if serde_json::to_writer(&mut *buf, v).is_err() {
                return Bytes::from_static(b"{}");
            }
            Bytes::copy_from_slice(&buf)
        }
        Err(_) => Bytes::from(serde_json::to_vec(v).unwrap_or_else(|_| b"{}".to_vec())),
    })
}

#[cfg(test)]
mod tests {
    use super::to_bytes;
    use serde::Serialize;

    #[derive(Serialize)]
    struct Sample {
        ok: bool,
        n: u32,
    }

    #[test]
    fn matches_serde_json_to_vec() {
        let s = Sample { ok: true, n: 7 };
        let a = to_bytes(&s);
        let b = serde_json::to_vec(&s).unwrap();
        assert_eq!(&a[..], b.as_slice());
    }
}
