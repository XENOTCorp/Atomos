//! First ATOMOS consumer: three JSON APIs + static files + pre/post + RAM state.
//!
//!   unset RUSTFLAGS
//!   export CARGO_TARGET_DIR=$HOME/.cache/atomos-target
//!   cargo run --release --example first_app -- 127.0.0.1:8090
//!
//! Notes live as an ArcSwap snapshot. GET never takes a mutex. POST serializes
//! writers with a mutex, then one store. Pinned epoll H1 workers.
//! Ports to skip come from host.json `refuse_ports`, not from this file.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use arc_swap::ArcSwap;
use atomos::config::Config;
use atomos::control_std;
use atomos::epoll;
use atomos::error::ServeError;
use atomos::flags::{FlagSet, FLAG_LOG};
use atomos::io::{Body, CacheDirective, In, Out};
use atomos::json_out;
use atomos::module::{Handler, Module};
use atomos::num::u64_to_slice;
use atomos::rules::Ruleset;
use atomos::status::Status;
use atomos::static_router;
use bytes::Bytes;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Serialize, Deserialize)]
struct Note {
    id: u64,
    text: String,
}

/// Immutable notes picture. GET copies `prefix` + itoa(hits) + `}`.
struct NotesSnap {
    notes: Vec<Note>,
    /// `{"notes":[...],"hits":`
    prefix: Bytes,
}

struct State {
    notes: ArcSwap<NotesSnap>,
    /// Only POSTs take this. GETs load the Arc.
    write: Mutex<()>,
    hits: AtomicU64,
    started: Instant,
    /// Clone of the kernel cache (shared epoch maps). POST invalidates "notes".
    cache: atomos::cache::ResponseCache,
}

fn build_snap(notes: Vec<Note>) -> NotesSnap {
    let mut prefix = Vec::with_capacity(32 + notes.len() * 48);
    prefix.extend_from_slice(br#"{"notes":"#);
    let _ = serde_json::to_writer(&mut prefix, &notes);
    prefix.extend_from_slice(br#","hits":"#);
    NotesSnap {
        notes,
        prefix: Bytes::from(prefix),
    }
}

fn notes_body(snap: &NotesSnap, hits: u64) -> Bytes {
    let mut nbuf = [0u8; 24];
    let n = u64_to_slice(hits, &mut nbuf);
    let mut v = Vec::with_capacity(snap.prefix.len() + n + 1);
    v.extend_from_slice(&snap.prefix);
    v.extend_from_slice(&nbuf[..n]);
    v.push(b'}');
    Bytes::from(v)
}

struct Pre {
    state: Arc<State>,
}

impl Module for Pre {
    fn name(&self) -> &'static str {
        "pre"
    }
    fn handle(&self, req: &In<'_>) -> Result<Out, ServeError> {
        self.state.hits.fetch_add(1, Ordering::Relaxed);
        if !req.peer.ip().is_loopback() {
            return Ok(Out::empty(Status::FORBIDDEN));
        }
        let mut out = Out::empty(Status::OK);
        let mut flags = FlagSet::empty();
        flags.insert(FLAG_LOG);
        out.flags = flags;
        Ok(out)
    }
}

struct Post;

impl Module for Post {
    fn name(&self) -> &'static str {
        "post"
    }
    fn handle(&self, _req: &In<'_>) -> Result<Out, ServeError> {
        let mut out = Out::empty(Status::OK);
        out.headers
            .push(("X-Atomos".into(), "first-app".into()));
        Ok(out)
    }
}

struct Api {
    state: Arc<State>,
}

impl Module for Api {
    fn name(&self) -> &'static str {
        "api"
    }
    fn handle(&self, req: &In<'_>) -> Result<Out, ServeError> {
        match (req.method, req.path) {
            (atomos::io::Method::Get, "/api/health") => {
                let mut out = Out::json(
                    Status::OK,
                    json_out::to_bytes(&json!({
                        "ok": true,
                        "uptime_ms": self.state.started.elapsed().as_millis() as u64
                    })),
                );
                out.cache = CacheDirective::Global { ttl_ms: 1_000 };
                Ok(out)
            }
            (atomos::io::Method::Get, "/api/notes") => {
                let snap = self.state.notes.load();
                let hits = self.state.hits.load(Ordering::Relaxed);
                let mut out = Out::json(Status::OK, notes_body(&snap, hits));
                out.cache = CacheDirective::Named {
                    ruleset: "notes".into(),
                    ttl_ms: 30_000,
                };
                Ok(out)
            }
            (atomos::io::Method::Post, "/api/notes") => {
                let raw = match req.body {
                    Body::Json(b) | Body::Raw(b) => b,
                    Body::Empty => b"",
                };
                let v: Value = serde_json::from_slice(raw).unwrap_or(json!({}));
                let text = v
                    .get("text")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if text.is_empty() {
                    return Ok(Out::json(
                        Status::BAD_REQUEST,
                        json_out::to_bytes(&json!({"ok": false, "error": "text required"})),
                    ));
                }
                if text.len() > 512 {
                    return Ok(Out::json(
                        Status::BAD_REQUEST,
                        json_out::to_bytes(&json!({"ok": false, "error": "text too long"})),
                    ));
                }
                let _g = self.state.write.lock();
                let cur = self.state.notes.load();
                if cur.notes.len() >= 256 {
                    return Ok(Out::json(
                        Status::from_u16(503),
                        json_out::to_bytes(&json!({"ok": false, "error": "cap"})),
                    ));
                }
                let mut notes = cur.notes.clone();
                let id = notes.last().map(|n| n.id + 1).unwrap_or(1);
                notes.push(Note {
                    id,
                    text: text.clone(),
                });
                self.state.notes.store(Arc::new(build_snap(notes)));
                self.state.cache.invalidate_named("notes");
                Ok(Out::json(
                    Status::CREATED,
                    json_out::to_bytes(&json!({"ok": true, "id": id, "text": text})),
                ))
            }
            _ => Err(ServeError::NoRule),
        }
    }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/first_app")
}

fn load_notes(path: &Path) -> Vec<Note> {
    let Ok(raw) = std::fs::read(path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_slice::<Value>(&raw) else {
        return Vec::new();
    };
    v.get("notes")
        .and_then(|n| serde_json::from_value(n.clone()).ok())
        .unwrap_or_default()
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let dir = root();
    let bind = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:8090".into());

    let notes_path = dir.join("notes.json");
    let notes0 = load_notes(&notes_path);

    let cfg_json = format!(
        r#"{{
            "bind":"{bind}",
            "cpu_pin":true,
            "engine":"epoll",
            "http2":false,
            "http3":false,
            "static_root":"{static_root}",
            "error_page":"{error}",
            "rules_path":"{rules}",
            "control_socket":"/tmp/atomos-first.sock",
            "memory_cap_bytes":67108864,
            "cache_entries":256,
            "cache_bytes":1048576,
            "so_reuseport":true,
            "tcp_nodelay":true,
            "pre_module":"pre",
            "post_module":"post"
        }}"#,
        bind = bind,
        static_root = dir.join("static").display(),
        error = dir.join("static/error.html").display(),
        rules = dir.join("rules.json").display(),
    );
    let mut cfg = Config::from_json(cfg_json.as_bytes()).unwrap_or_else(|e| {
        eprintln!("config: {e}");
        std::process::exit(1);
    });
    cfg.apply_host_file();
    if let Ok(p) = cfg.port() {
        if cfg.refuse_ports.contains(&p) {
            eprintln!("bind port {p} is in refuse_ports (host overlay)");
            std::process::exit(2);
        }
    }
    let rules_raw = std::fs::read(&cfg.rules_path).unwrap_or_else(|e| {
        eprintln!("rules {}: {e}", cfg.rules_path.display());
        std::process::exit(1);
    });
    let rules = Ruleset::parse(&rules_raw).unwrap_or_else(|e| {
        eprintln!("rules: {e}");
        std::process::exit(1);
    });

    let sock = cfg.control_socket.clone();
    let (mut router, ctx, _) = static_router(cfg, rules);
    let state = Arc::new(State {
        notes: ArcSwap::from_pointee(build_snap(notes0)),
        write: Mutex::new(()),
        hits: AtomicU64::new(0),
        started: Instant::now(),
        cache: router.cache.clone(),
    });
    router.insert(
        "api",
        Handler::Sync(Arc::new(Api {
            state: state.clone(),
        })),
    );
    router.insert(
        "pre",
        Handler::Sync(Arc::new(Pre {
            state: state.clone(),
        })),
    );
    router.insert("post", Handler::Sync(Arc::new(Post)));
    {
        let r = Arc::get_mut(&mut router).expect("unique");
        r.bind_hooks();
    }
    let ctl = ctx.clone();
    std::thread::spawn(move || {
        if let Err(e) = control_std::serve_control(sock, ctl) {
            tracing::warn!(%e, "control socket");
        }
    });
    tracing::info!("first_app  GET /api/health  GET /api/notes  POST /api/notes  engine=epoll");
    if let Err(e) = epoll::run(router, ctx) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
