//! Cached error HTML. Placeholders {{code}} {{phrase}} {{detail}}.

use bytes::Bytes;

use crate::num::u16_to_slice;
use crate::status::Status;

const FALLBACK: &str = include_str!("error.html");

#[derive(Clone)]
pub struct ErrorPage {
    tmpl: String,
}

impl ErrorPage {
    pub fn load(path: &std::path::Path) -> Self {
        let tmpl = std::fs::read_to_string(path).unwrap_or_else(|_| FALLBACK.to_string());
        Self { tmpl }
    }

    pub fn builtin() -> Self {
        Self {
            tmpl: FALLBACK.to_string(),
        }
    }

    pub fn render(&self, status: Status, detail: &str) -> Bytes {
        let mut code = [0u8; 8];
        let n = u16_to_slice(status.as_u16(), &mut code);
        let code = std::str::from_utf8(&code[..n]).unwrap_or("000");
        let html = self
            .tmpl
            .replace("{{code}}", code)
            .replace("{{phrase}}", status.phrase())
            .replace("{{detail}}", detail);
        Bytes::from(html)
    }
}
