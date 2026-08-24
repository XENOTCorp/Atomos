//! Static files. Path traversal rejected. `/` → index.html.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bytes::Bytes;

use crate::error::ServeError;
use crate::error_page::ErrorPage;
use crate::io::{CacheDirective, In, Method, Out, OutBody};
use crate::mime;
use crate::module::Module;
use crate::status::Status;

pub struct StaticMod {
    root: PathBuf,
    errors: ErrorPage,
    pub hits: AtomicU64,
}

impl StaticMod {
    pub fn new(root: PathBuf, errors: ErrorPage) -> Arc<Self> {
        Arc::new(Self {
            root,
            errors,
            hits: AtomicU64::new(0),
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
        self.hits.fetch_add(1, Ordering::Relaxed);
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
        let dest = if !dest.exists() && dest.extension().is_none() {
            dest.with_extension("html")
        } else {
            dest
        };
        let bytes = match std::fs::read(&dest) {
            Ok(b) => b,
            Err(_) => return Ok(self.not_found()),
        };
        let ct = mime::from_path(dest.to_str().unwrap_or(rel));
        let ttl = if ct.starts_with("text/html") {
            5_000
        } else {
            60_000
        };
        let body = if req.method == Method::Head {
            OutBody::Empty
        } else {
            OutBody::Raw(Bytes::from(bytes))
        };
        Ok(Out {
            status: Status::OK,
            reason: None,
            headers: vec![("Content-Type".into(), ct.into())],
            body,
            cache: CacheDirective::Global { ttl_ms: ttl },
            flags: crate::flags::FlagSet::empty(),
        })
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
}
