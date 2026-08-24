//! Generic request/response. Strings on `In` borrow the receive buffer.
//!
//! `Out` fields modules may set (examples):
//! - `status`: `Status::OK` or `Status::NOT_FOUND`
//! - `reason`: None → RFC phrase; Some only to override
//! - `headers`: extra Content-Type, ETag
//! - `body`: Raw for files, Json for APIs (already serialized)
//! - `cache`: No (default) | Global { ttl_ms } | Named { id, ttl_ms }
//! - `flags`: passed to post-module (`FLAG_LOG`, `FLAG_METRICS_SKIP`, `FLAG_NO_POST`)

use std::net::SocketAddr;

use bytes::Bytes;

use crate::flags::FlagSet;
use crate::status::Status;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Head,
    Post,
    Put,
    Delete,
    Patch,
    Options,
    Trace,
    Connect,
}

impl Method {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "GET" => Method::Get,
            "HEAD" => Method::Head,
            "POST" => Method::Post,
            "PUT" => Method::Put,
            "DELETE" => Method::Delete,
            "PATCH" => Method::Patch,
            "OPTIONS" => Method::Options,
            "TRACE" => Method::Trace,
            "CONNECT" => Method::Connect,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Head => "HEAD",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Patch => "PATCH",
            Method::Options => "OPTIONS",
            Method::Trace => "TRACE",
            Method::Connect => "CONNECT",
        }
    }

    /// Bit in a ruleset method mask. `0xFFFF` in a rule means all methods.
    pub const fn bit(self) -> u16 {
        match self {
            Method::Get => 1 << 0,
            Method::Head => 1 << 1,
            Method::Post => 1 << 2,
            Method::Put => 1 << 3,
            Method::Delete => 1 << 4,
            Method::Patch => 1 << 5,
            Method::Options => 1 << 6,
            Method::Trace => 1 << 7,
            Method::Connect => 1 << 8,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HeaderView<'buf> {
    pub pairs: Vec<(&'buf str, &'buf str)>,
}

impl<'buf> HeaderView<'buf> {
    pub fn get(&self, name: &str) -> Option<&'buf str> {
        self.pairs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| *v)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Body<'buf> {
    Empty,
    Raw(&'buf [u8]),
    /// First-byte `{` or `[` passed; depth/size already checked.
    Json(&'buf [u8]),
}

/// Domain: one HTTP request whose bytes still live in `buf`.
/// Invariant: path/query/headers/body borrow `buf`.
pub struct In<'buf> {
    pub method: Method,
    pub path: &'buf str,
    pub query: &'buf str,
    pub headers: HeaderView<'buf>,
    pub body: Body<'buf>,
    pub peer: SocketAddr,
    pub flags: FlagSet,
}

#[derive(Clone, Debug)]
pub enum OutBody {
    Empty,
    Raw(Bytes),
    Json(Bytes),
    /// Response body streamed as chunks arrive (tokio path; the H1
    /// epoll path encodes it as empty — streaming modules are async and
    /// never dispatch on the sync H1 loop).
    Stream(StreamBody),
    /// Open file range. The H1 epoll path sends it with `sendfile`
    /// (zero-copy in kernel); the tokio paths materialize it via
    /// [`FileBody::read_to_bytes`] (H2/H3 framing and TLS need the
    /// bytes in memory). Never stored in the response cache.
    File(FileBody),
}

/// Open file range to be sent with `sendfile` on the H1 epoll path.
/// The fd stays open for the lifetime of the `Arc` — `StaticMod` keeps
/// a bounded LRU of these (the open_file_cache equivalent; stale after
/// on-disk replace until evicted, same as nginx).
#[derive(Clone, Debug)]
pub struct FileBody {
    pub file: std::sync::Arc<std::fs::File>,
    pub offset: u64,
    pub len: u64,
}

impl FileBody {
    /// Blocking `pread` of the whole range into memory. Used by the
    /// tokio paths (H1/H2/H3), which cannot sendfile: framing and TLS
    /// need the bytes in memory. Callers on a tokio worker should run
    /// this on a blocking thread (`tokio::task::spawn_blocking`).
    pub fn read_to_bytes(&self) -> std::io::Result<Bytes> {
        use std::os::unix::fs::FileExt;
        let mut buf = vec![0u8; self.len as usize];
        let mut got = 0usize;
        while got < buf.len() {
            let n = self.file.read_at(&mut buf[got..], self.offset + got as u64)?;
            if n == 0 {
                break;
            }
            got += n;
        }
        buf.truncate(got);
        Ok(Bytes::from(buf))
    }
}

/// Chunk receiver for a streaming response body. Wrapped so `Out` stays
/// `Clone` (the cache clones `Out`); take it exactly once.
#[derive(Clone, Debug)]
pub struct StreamBody(pub std::sync::Arc<parking_lot::Mutex<Option<tokio::sync::mpsc::Receiver<Bytes>>>>);

impl StreamBody {
    pub fn take(&self) -> tokio::sync::mpsc::Receiver<Bytes> {
        self.0
            .lock()
            .take()
            .expect("StreamBody taken more than once")
    }
}

impl OutBody {
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            OutBody::Empty => b"",
            OutBody::Raw(b) | OutBody::Json(b) => b,
            OutBody::Stream(_) | OutBody::File(_) => b"",
        }
    }

    pub fn len(&self) -> usize {
        match self {
            OutBody::Stream(_) => 0,
            OutBody::File(f) => f.len as usize,
            _ => self.as_bytes().len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Cheap owned body bytes when the body is already in memory: the
    /// `Bytes` clone is a refcount bump, not a copy. `Empty`/`Stream`
    /// return `None`; `File` returns `None` (materialize it explicitly
    /// via [`FileBody::read_to_bytes`]).
    pub fn to_bytes(&self) -> Option<Bytes> {
        match self {
            OutBody::Raw(b) | OutBody::Json(b) => Some(b.clone()),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheDirective {
    No,
    Global { ttl_ms: u32 },
    Named { ruleset: Box<str>, ttl_ms: u32 },
}

/// Domain: one module result. See module docs for field examples.
#[derive(Clone, Debug)]
pub struct Out {
    pub status: Status,
    pub reason: Option<Box<str>>,
    pub headers: Vec<(Box<str>, Box<str>)>,
    pub body: OutBody,
    pub cache: CacheDirective,
    pub flags: FlagSet,
}

impl In<'_> {
    pub fn to_owned(&self) -> InOwned {
        let body = match self.body {
            Body::Empty => Vec::new(),
            Body::Raw(b) | Body::Json(b) => b.to_vec(),
        };
        InOwned {
            method: self.method,
            path: self.path.to_string(),
            query: self.query.to_string(),
            headers: self
                .headers
                .pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            body,
            peer: self.peer,
            flags: self.flags,
        }
    }
}

impl InOwned {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

impl Out {
    pub fn empty(status: Status) -> Self {
        Self {
            status,
            reason: None,
            headers: Vec::new(),
            body: OutBody::Empty,
            cache: CacheDirective::No,
            flags: FlagSet::empty(),
        }
    }

    pub fn raw(status: Status, body: Bytes, content_type: &str) -> Self {
        Self {
            status,
            reason: None,
            headers: vec![("Content-Type".into(), content_type.into())],
            body: OutBody::Raw(body),
            cache: CacheDirective::No,
            flags: FlagSet::empty(),
        }
    }

    pub fn json(status: Status, body: Bytes) -> Self {
        Self {
            status,
            reason: None,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: OutBody::Json(body),
            cache: CacheDirective::No,
            flags: FlagSet::empty(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct InOwned {
    pub method: Method,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub peer: SocketAddr,
    pub flags: FlagSet,
}
