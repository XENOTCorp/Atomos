//! Simple API login server: a module-based example.
//!
//!   - `POST /api/login` with `{"user","pass"}` returns a bearer token.
//!   - `GET /api/session` with `Authorization: Bearer <token>` returns
//!     the authenticated user.
//!   - Static files are served for every other path.
//!
//! The token is a hash of the credentials and a request counter. This
//! example is a demonstration of the module API, not a security design.
//!
//! ```sh
//! cargo run --release --example login_server -- 127.0.0.1:8090
//! curl -X POST -d '{"user":"alice","pass":"wonderland"}' http://127.0.0.1:8090/api/login
//! curl -H 'Authorization: Bearer <token>' http://127.0.0.1:8090/api/session
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use atomos::config::Config;
use atomos::error::ServeError;
use atomos::flags::{FlagSet, FLAG_LOG};
use atomos::io::{Body, In, Method, Out};
use atomos::json_out;
use atomos::module::{Handler, Module};
use atomos::status::Status;
use atomos::static_router;
use serde_json::{json, Value};

/// Demo credential table. A real deployment reads this from a file or a
/// key-value store at startup.
const USERS: &[(&str, &str)] = &[("alice", "wonderland"), ("bob", "builder")];

/// token -> user. GET reads the Arc; POST stores a new Arc (lock-free).
struct Sessions {
    map: ArcSwap<HashMap<String, String>>,
    counter: AtomicU64,
}

fn token_for(user: &str, pass: &str, n: u64) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    user.hash(&mut h);
    pass.hash(&mut h);
    n.hash(&mut h);
    format!("{:016x}", h.finish())
}

struct Login {
    sessions: Arc<Sessions>,
}

impl Module for Login {
    fn name(&self) -> &'static str {
        "login"
    }
    fn handle(&self, req: &In<'_>) -> Result<Out, ServeError> {
        if req.method != Method::Post {
            return Ok(Out::json(
                Status::METHOD_NOT_ALLOWED,
                json_out::to_bytes(&json!({"ok": false, "error": "method"})),
            ));
        }
        let raw = match req.body {
            Body::Json(b) | Body::Raw(b) => b,
            Body::Empty => b"",
        };
        let v: Value = serde_json::from_slice(raw).unwrap_or(json!({}));
        let user = v.get("user").and_then(Value::as_str).unwrap_or("");
        let pass = v.get("pass").and_then(Value::as_str).unwrap_or("");
        if !USERS.iter().any(|(u, p)| *u == user && *p == pass) {
            return Ok(Out::json(
                Status::UNAUTHORIZED,
                json_out::to_bytes(&json!({"ok": false, "error": "bad credentials"})),
            ));
        }
        let n = self.sessions.counter.fetch_add(1, Ordering::Relaxed);
        let token = token_for(user, pass, n);
        let mut map = (**self.sessions.map.load()).clone();
        map.insert(token.clone(), user.to_string());
        self.sessions.map.store(Arc::new(map));
        Ok(Out::json(
            Status::OK,
            json_out::to_bytes(&json!({"ok": true, "token": token})),
        ))
    }
}

struct Session {
    sessions: Arc<Sessions>,
}

impl Module for Session {
    fn name(&self) -> &'static str {
        "session"
    }
    fn handle(&self, req: &In<'_>) -> Result<Out, ServeError> {
        if req.method != Method::Get {
            return Ok(Out::json(
                Status::METHOD_NOT_ALLOWED,
                json_out::to_bytes(&json!({"ok": false, "error": "method"})),
            ));
        }
        let bearer = req
            .headers
            .get("authorization")
            .and_then(|h| h.strip_prefix("Bearer "))
            .unwrap_or("");
        match self.sessions.map.load().get(bearer) {
            Some(user) => Ok(Out::json(
                Status::OK,
                json_out::to_bytes(&json!({"ok": true, "user": user})),
            )),
            None => Ok(Out::json(
                Status::UNAUTHORIZED,
                json_out::to_bytes(&json!({"ok": false, "error": "invalid token"})),
            )),
        }
    }
}

/// Loopback-only guard with request logging (pre module).
struct Pre;

impl Module for Pre {
    fn name(&self) -> &'static str {
        "pre"
    }
    fn handle(&self, req: &In<'_>) -> Result<Out, ServeError> {
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

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/login_server")
}

fn main() {
    let dir = root();
    let bind = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:8090".into());
    let sessions = Arc::new(Sessions {
        map: ArcSwap::from_pointee(HashMap::new()),
        counter: AtomicU64::new(0),
    });

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
            "control_socket":"/tmp/atomos-login.sock",
            "memory_cap_bytes":67108864,
            "cache_entries":256,
            "cache_bytes":1048576,
            "so_reuseport":true,
            "tcp_nodelay":true,
            "pre_module":"pre"
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
    let rules = atomos::rules::Ruleset::parse(&rules_raw).unwrap_or_else(|e| {
        eprintln!("rules: {e}");
        std::process::exit(1);
    });

    let (mut router, ctx, _) = static_router(cfg, rules);
    router.insert(
        "login",
        Handler::Sync(Arc::new(Login {
            sessions: sessions.clone(),
        })),
    );
    router.insert("session", Handler::Sync(Arc::new(Session { sessions })));
    router.insert("pre", Handler::Sync(Arc::new(Pre)));
    {
        let r = Arc::get_mut(&mut router).expect("unique");
        r.bind_hooks();
    }
    atomos::epoll::run(router, ctx).expect("server");
}
