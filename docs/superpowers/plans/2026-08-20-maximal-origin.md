# Maximal origin occupancy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Inhabit every ARCSS-admissible axis of site S (loopback kernel-TCP origin): one concurrency model per process, TCB-split H1 vs proto binaries, jail leftovers, key process, Wasm off cache-hit, effect adapters — without `.so`/Lua and without leaving S.

**Architecture:** Coproduct of processes (`atomos` epoll H1 ⊔ `atomos-proto` tokio H2/H3/TLS ⊔ `atomos-sup` ⊔ `atomos-keyd`). Kernel `handle: In → Out` is the unique natural transformation across wire representations. Cache hit is H-07 memoization (no plugin). `.so`/Lua refused (gain theorem in the spec).

**Tech Stack:** Existing Atomos crate (Rust 1.80, four planes). Optional features `proto` (h2, h3, quinn, rustls) and `wasm` (wasmtime). No new C TCB. Broadwell AVX2 via `scripts/cpu-rustflags.sh`.

**Spec:** `docs/superpowers/specs/2026-08-20-maximal-origin-design.md`

## Global Constraints

- Criticality C2; `#![deny(warnings)]`; clippy `-D warnings`.
- `CARGO_TARGET_DIR=$HOME/.cache/atomos-target`; unset host `RUSTFLAGS` if they inject clang/mold; `/tmp` is noexec.
- Default bind `127.0.0.1:8090`. Never bind `8082`. Never overwrite `/usr/local/bin/paper-retrieval`. Do not restart `~/boot/35-papers`. Do not run `atomos-ctl install-link` unless the user asks.
- Tests use `127.0.0.1:0` or an ephemeral port.
- `panic=abort`. No regex routers, no io_uring, no software prefetch, no simd-json, no `.so`, no Lua.
- Invariants `I_bind` `I_engine` `I_hit` `I_get` `I_cache` `I_cc00` `I_cc02` `I_cap` `I_tcb` `I_jail` `I_rss` `I_pure` from the spec.
- Four planes: new code in the plane that owns the morphism (`kernel/` no sockets, `net/` engines, `ops/` sup/jail/ctl, `plugin/` manifests+Wasm).

## File map

| File | Responsibility |
|---|---|
| `src/kernel/config.rs` | Default `engine=epoll`; reject epoll∧(h2∨h3); `worker_shutdown_timeout_ms`; `landlock`/`seccomp` flags |
| `src/ops/atom.rs` | `AtomCtx` atomics only — no `tokio::sync::Notify` |
| `src/net/engine.rs` | Dispatch: epoll blocking; proto async under feature |
| `src/net/epoll.rs` | `run` blocking join of pinned threads |
| `src/net/serve.rs` `h2serve.rs` `h3serve.rs` `tls.rs` | `#[cfg(feature = "proto")]` |
| `src/bin/serve.rs` | H1-only `main` (std, no `#[tokio::main]`) |
| `src/bin/proto.rs` | New: `atomos-proto` tokio main |
| `src/bin/keyd.rs` | New: `atomos-keyd` |
| `src/ops/sup.rs` | Two-generation SIGHUP, drain timeout, heartbeat |
| `src/ops/jail.rs` | Landlock, seccomp, capset |
| `src/ops/control.rs` | Std blocking Unix server on H1; tokio under proto |
| `src/plugin/wasm.rs` | New: wasmtime + fuel + epoch; never called from cache hit |
| `src/plugin/registry.rs` | Load wasm when feature on |
| `src/kernel/metrics.rs` | New: atomic counters; `/metrics` is a Module |
| `src/net/access_log.rs` | New: `Log` after encode |
| `Cargo.toml` | Features `h1` (default), `proto`, `wasm`; bins |
| `docs/h04-matrix.md` | H-04 decision matrix (gain theorem numbers) |
| `docs/architecture.md` `planes.md` `lack.md` `scorecard.md` | Declare models; recategorize only after tests pass |
| `tests/smuggling.rs` `fuzz/` | Domain tests |

---

### Task 1: Coproduct of engines in Config (I_engine)

**Files:**
- Modify: `src/kernel/config.rs` (`default_engine`, `validate`)
- Modify: `src/net/engine.rs` (comment + default `EngineKind`)
- Create: `docs/h04-matrix.md`
- Test: existing unit tests in `src/kernel/config.rs` if present; add tests next to `validate` in `config.rs`

**Interfaces:**
- Consumes: `Config.engine: String`, `http2: bool`, `http3: bool`
- Produces: `default_engine() -> "epoll"`; `validate()` returns `Err(ServeError::Config)` when `EngineKind::parse(engine)==Some(Epoll)` and (`http2` or `http3`); `EngineKind` default `Epoll`

- [ ] **Step 1: Write the failing tests**

Add at the bottom of `src/kernel/config.rs` in the existing `#[cfg(test)]` module, or create one if absent:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_engine_is_epoll() {
        let c = Config::from_json(br#"{"bind":"127.0.0.1:0","memory_cap_bytes":67108864}"#).unwrap();
        assert_eq!(c.engine, "epoll");
    }

    #[test]
    fn epoll_with_http2_is_config_error() {
        let e = Config::from_json(
            br#"{"bind":"127.0.0.1:0","memory_cap_bytes":67108864,"engine":"epoll","http2":true,"http3":false}"#,
        )
        .unwrap_err();
        let s = e.to_string();
        assert!(s.contains("epoll"), "{s}");
        assert!(s.contains("http2") || s.contains("engine"), "{s}");
    }

    #[test]
    fn tokio_may_enable_http2() {
        Config::from_json(
            br#"{"bind":"127.0.0.1:0","memory_cap_bytes":67108864,"engine":"tokio","http2":true,"http3":false}"#,
        )
        .unwrap();
    }
}
```

Also add in `src/net/engine.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_kind_is_epoll() {
        assert_eq!(EngineKind::default(), EngineKind::Epoll);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
unset RUSTFLAGS
export CARGO_TARGET_DIR=$HOME/.cache/atomos-target
cd /home/xenot/Projects/Atomos
cargo test --lib config::tests::default_engine_is_epoll engine::tests::default_kind_is_epoll -- --nocapture
```

Expected: FAIL (`engine` is `"tokio"` / default `Tokio`).

- [ ] **Step 3: Minimal implementation**

In `src/kernel/config.rs`:

```rust
fn default_engine() -> String {
    "epoll".into()
}
```

In `validate`, after the tls_cert/tls_key match:

```rust
let kind = crate::engine::EngineKind::parse(&self.engine);
if kind.is_none() {
    return Err(ServeError::Config("unknown engine".into()));
}
if kind == Some(crate::engine::EngineKind::Epoll) && (self.http2 || self.http3) {
    return Err(ServeError::Config(
        "engine=epoll cannot set http2/http3 (I_engine; use atomos-proto)".into(),
    ));
}
if kind == Some(crate::engine::EngineKind::Xdp) {
    return Err(ServeError::Config("engine xdp is not linked in this site".into()));
}
```

To avoid a kernel→net dependency, move `EngineKind::parse` string check inline in `validate` (`"epoll"|"tokio"|"xdp"|"af-xdp"`) — **kernel must not import `net`**. Duplicate the three names as a `fn engine_ok(s: &str) -> bool` in `config.rs`. Keep `EngineKind` in `net/engine.rs`.

In `src/net/engine.rs`:

```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EngineKind {
    #[default]
    Epoll,
    Tokio,
    Xdp,
}
```

Fix `default_http2` / `default_http3`: they must become `false` so a missing-field JSON still validates with default epoll. Proto binary (Task 3) sets them true in its own default JSON.

```rust
fn default_false() -> bool { false }
```

Change `http2` and `http3` serde defaults from `default_true` to `default_false`.

Write `docs/h04-matrix.md` with the table from spec §5 (`T_sys` ≳ 18 µs, `p_hit` → 1, `.so` gain → 0, prefetch/io_uring/simd-json rejected).

- [ ] **Step 4: Run tests and fix fallout**

```bash
unset RUSTFLAGS
export CARGO_TARGET_DIR=$HOME/.cache/atomos-target
cargo test --lib
```

Expected: existing tests that omitted `engine` now get epoll. Any test that needs tokio H2 must set `"engine":"tokio"`. `examples/config.json` currently has `"http2": true, "http3": true` without engine — either add `"engine":"tokio"` (proto example) or drop http2/http3 (H1 example). Split: keep `examples/config.json` as H1 (`engine: epoll`, http2/http3 false); proto example later.

`tests/http2_h3.rs` must pass `"engine":"tokio"`.

- [ ] **Step 5: Commit**

```bash
git add src/kernel/config.rs src/net/engine.rs docs/h04-matrix.md examples/config.json tests/http2_h3.rs
git commit -m "feat: default engine epoll; reject epoll with http2/http3 (I_engine)"
```

---

### Task 2: AtomCtx without tokio; blocking epoll run (I_cc00)

**Files:**
- Modify: `src/ops/atom.rs` (drop `Notify` from `AtomCtx`)
- Modify: `src/lib.rs` (`static_router` construction)
- Modify: `src/net/epoll.rs` (`pub fn run` blocking; join threads)
- Modify: `src/net/engine.rs` (`Epoll` branch not async)
- Modify: `src/net/serve.rs` `src/net/h3serve.rs` (local `Notify` or poll `stop`)
- Modify: `tests/epoll_smoke.rs`
- Modify: `src/bin/serve.rs` — wait for Task 3 if still `#[tokio::main]`; this task may keep an async wrapper in `engine::run` that only `spawn_blocking`s epoll, then Task 3 removes it

**Interfaces:**
- Consumes: `AtomCtx { stop: Arc<LineAtomicU8>, ... }`
- Produces: `AtomCtx` with **no** `wake` field. Atoms that used `ctx.wake.notify_waiters()` store `ctx.stop` or a new `Arc<LineAtomicU8> reload` if needed. Epoll: `pub fn run(router: Arc<Router>, ctx: Arc<AtomCtx>) -> Result<(), ServeError>` (blocking). Proto serve keeps its own `tokio::sync::Notify` inside `net/serve.rs` as `struct ProtoWake(Arc<Notify>)` not on `AtomCtx`.

- [ ] **Step 1: Write the failing test**

In `src/ops/atom.rs` tests (add module if needed):

```rust
#[test]
fn atom_ctx_has_no_tokio_wake() {
    let ctx = AtomCtx::test();
    let _ = ctx.stop.v.load(std::sync::atomic::Ordering::Acquire);
    // compile-time: AtomCtx must not have a field `wake`.
}
```

This test stays as a compile probe: after removing `wake`, any remaining `ctx.wake` fails compile — that **is** the test. Also add in `src/net/epoll.rs`:

```rust
#[test]
fn epoll_run_is_blocking_signature() {
    // If this compiles, run is fn not async fn.
    let _: fn(Arc<crate::route::Router>, Arc<crate::atom::AtomCtx>) -> Result<(), crate::error::ServeError> =
        run;
}
```

- [ ] **Step 2: Run to see compile/test fail**

```bash
cargo test --lib epoll::tests::epoll_run_is_blocking_signature
```

Expected: FAIL (current `run` is `async fn`).

- [ ] **Step 3: Implementation**

`AtomCtx` — remove `wake`. In `dispatch` paths that called `notify_waiters` (stop/restart atoms around lines 160 and 164 of `atom.rs`), only `stop.v.store(...)`.

`epoll.rs`:

```rust
pub fn run(router: Arc<Router>, ctx: Arc<AtomCtx>) -> Result<(), ServeError> {
    // existing bind + jail::after_bind ...
    let mut joins = Vec::new();
    for i in 0..n {
        let tcp = tcps.remove(0);
        let router = router.clone();
        let ctx = ctx.clone();
        let h = std::thread::Builder::new()
            .name(format!("atomos-epoll-{i}"))
            .spawn(move || {
                if router.cfg.cpu_pin {
                    let _ = pin_cpu::pin_to_cpu(i as usize);
                }
                worker(tcp.listener, router, ctx)
            })
            .map_err(ServeError::Io)?;
        joins.push(h);
    }
    for h in joins {
        let _ = h.join();
    }
    Ok(())
}
```

Delete `tokio::sync::oneshot` from this file.

`engine.rs`:

```rust
pub async fn run(
    kind: EngineKind,
    router: Arc<Router>,
    ctx: Arc<AtomCtx>,
) -> Result<(), ServeError> {
    match kind {
        EngineKind::Epoll => {
            tokio::task::spawn_blocking(move || super::epoll::run(router, ctx))
                .await
                .map_err(|e| ServeError::Io(std::io::Error::other(e)))?
        }
        EngineKind::Tokio => serve::run(router, ctx).await,
        EngineKind::Xdp => Err(ServeError::Config(
            "engine xdp is not linked in this build".into(),
        )),
    }
}
```

The `spawn_blocking` wrapper is a **temporary** adapter so `#[tokio::test]` smoke still works. Task 3 removes `engine::run` async for H1 and makes `atomos` a std main.

`serve.rs` / `h3serve.rs`: replace `ctx.wake.notified()` with `tokio::time::sleep` polling `ctx.stop` **or** a `Notify` owned by the proto engine:

```rust
// net/serve.rs
struct ProtoCtl {
    stop: Arc<LineAtomicU8>,
    wake: Arc<tokio::sync::Notify>,
}
```

Do **not** put `Notify` back on `AtomCtx`. If stop-atoms must interrupt proto immediately, add `ops/atom.rs` optional callback later; polling 200 ms matches epoll and is a bound, not a heuristic.

Update `static_router` in `src/lib.rs` to not set `wake`.

`tests/epoll_smoke.rs`: spawn a std thread:

```rust
std::thread::spawn(move || {
    let _ = atomos::net::epoll_run(router, ctx); // re-export
});
```

Re-export `pub use net::epoll::run as epoll_run` from `lib.rs` or `engine::run_epoll`.

- [ ] **Step 4: Test**

```bash
unset RUSTFLAGS
export CARGO_TARGET_DIR=$HOME/.cache/atomos-target
cargo test --lib
cargo test --test epoll_smoke --test http_smoke --test json_bomb --test rules_dry
```

Expected: PASS. `http2_h3` still uses tokio engine.

- [ ] **Step 5: Commit**

```bash
git add src/ops/atom.rs src/lib.rs src/net/epoll.rs src/net/engine.rs src/net/serve.rs src/net/h3serve.rs tests/epoll_smoke.rs
git commit -m "refactor: AtomCtx atomics only; epoll run is blocking (CC-00)"
```

---

### Task 3: TCB split — features `proto` / `wasm`, bin `atomos-proto` (I_tcb)

**Files:**
- Modify: `Cargo.toml`
- Create: `src/bin/proto.rs`
- Modify: `src/bin/serve.rs` (std `fn main`, call blocking epoll)
- Modify: `src/lib.rs` (cfg proto re-exports)
- Modify: `src/net/mod.rs` (`#[cfg(feature = "proto")]` on h2/h3/tls/serve)
- Modify: `src/ops/control.rs` — split std control server for H1
- Create: `src/ops/control_std.rs` (blocking UnixListener)
- Modify: tests `http2_h3.rs` → `cargo test --features proto --test http2_h3`
- Modify: `examples/first_app.rs` if it enables h2/h3 — `--features proto` in its harness docs

**Interfaces:**
- Consumes: Task 1–2 types
- Produces:
  - features: `default = ["h1"]`, `h1 = []`, `proto = ["dep:h2","dep:h3","dep:h3-quinn","dep:quinn","dep:rustls","dep:rustls-pemfile","dep:tokio-rustls","dep:rcgen"]`, `wasm = ["dep:wasmtime"]`
  - `[[bin]] name = "atomos-proto" path = "src/bin/proto.rs" required-features = ["proto"]`
  - `atomos` binary: no `#[tokio::main]`; `epoll::run`; `control_std::serve`
  - Default `cargo build --release` does **not** link quinn (check `ldd`/`cargo tree -i quinn` fails)

- [ ] **Step 1: Write the failing TCB test (script)**

Create `tests/tcb_h1.rs` as a build-info unit that is always compiled, plus a shell assertion in this step:

```rust
#[test]
fn h1_build_does_not_export_h2serve() {
    // Without feature proto, crate::h2serve must not exist.
    // This file is compiled without proto in default cargo test.
}
```

The real gate is:

```bash
unset RUSTFLAGS
export CARGO_TARGET_DIR=$HOME/.cache/atomos-target
cd /home/xenot/Projects/Atomos
cargo tree -p atomos --edges normal -i quinn
```

Expected **after** this task: error `package ID specified did not match any packages`. **Before:** quinn is in the tree. Document that as the failing probe.

- [ ] **Step 2: Run probe (quinn is present)**

```bash
cargo tree -p atomos -i quinn
```

Expected: prints quinn (current). That is the fail.

- [ ] **Step 3: Cargo.toml + cfg**

Make optional:

```toml
h2 = { version = "0.4", optional = true }
h3 = { version = "0.0.8", optional = true }
h3-quinn = { version = "0.0.10", optional = true }
quinn = { version = "0.11", default-features = false, features = ["runtime-tokio", "rustls-ring"], optional = true }
rcgen = { version = "0.13", default-features = false, features = ["ring", "pem"], optional = true }
rustls = { version = "0.23", default-features = false, features = ["std", "ring", "tls12", "logging"], optional = true }
rustls-pemfile = { version = "2", optional = true }
tokio-rustls = { version = "0.26", default-features = false, features = ["ring", "tls12", "logging"], optional = true }

[features]
default = ["h1"]
h1 = []
proto = ["h2", "h3", "h3-quinn", "quinn", "rcgen", "rustls", "rustls-pemfile", "tokio-rustls"]
wasm = []

[[bin]]
name = "atomos-proto"
path = "src/bin/proto.rs"
required-features = ["proto"]
```

Keep `tokio` for now on the lib if proto tests need it; **H1 bin must not use it**. If tokio remains a lib dep, `cargo tree -i quinn` is the TCB test, not “zero tokio in Cargo.lock”. Unique L-02 for H1 is **no quinn/h2/h3/rustls**. Tokio in the lib is allowed until control_std is done; then tokio can be optional too:

```toml
tokio = { version = "1", optional = true, features = ["rt-multi-thread", "macros", "net", "signal", "sync", "time", "fs", "io-util"] }
# proto = [..., "dep:tokio"]
```

Do optional tokio in this task if compile cost is acceptable; otherwise a follow-up commit in the same task after control_std exists.

`src/net/mod.rs`:

```rust
pub mod encode;
pub mod engine;
pub(crate) mod epoll;
#[cfg(feature = "proto")]
pub(crate) mod h2serve;
#[cfg(feature = "proto")]
pub(crate) mod h3serve;
pub mod listen;
pub mod parse;
pub(crate) mod pin_cpu;
#[cfg(feature = "proto")]
pub(crate) mod proto;
#[cfg(feature = "proto")]
pub mod serve;
#[cfg(feature = "proto")]
pub(crate) mod tls;
```

`src/lib.rs`: wrap `pub(crate) use net::h2serve` etc. in `#[cfg(feature = "proto")]`.

`src/ops/control_std.rs` (new):

```rust
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::Arc;
use crate::atom::{dispatch, AtomCtx};
use crate::error::ServeError;
use crate::jail;

pub fn serve_control(path: std::path::PathBuf, ctx: Arc<AtomCtx>) -> Result<(), ServeError> {
    jail::prepare_socket_dir(&path)?;
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    listener.set_nonblocking(false)?;
    loop {
        if ctx.stop.v.load(std::sync::atomic::Ordering::Acquire) != 0 {
            break;
        }
        let (sock, _) = match listener.accept() {
            Ok(s) => s,
            Err(e) => return Err(e.into()),
        };
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            if !jail::peer_euid_ok(sock.as_raw_fd()) {
                continue;
            }
        }
        let mut r = BufReader::new(sock.try_clone()?);
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(&line).unwrap_or_else(|_| serde_json::json!({"ok":false}));
        let name = v.get("cmd").and_then(|c| c.as_str()).unwrap_or("");
        let out = dispatch(&ctx, name, v).unwrap_or_else(|e| serde_json::json!({"ok":false,"error":e.to_string()}));
        let mut sock = r.into_inner();
        let _ = writeln!(sock, "{out}");
    }
    Ok(())
}
```

Match the existing JSON-lines protocol in `control.rs` exactly — read `src/ops/control.rs` fully and copy the command dispatch, only replacing `tokio::net::UnixListener` with `std::os::unix::net::UnixListener`. Do not invent a new schema.

`src/bin/serve.rs`:

```rust
fn main() {
    // tracing init as today
    // load config/rules as today
    let kind = atomos::engine::EngineKind::parse(&cfg.engine).unwrap_or_default();
    if kind != atomos::engine::EngineKind::Epoll {
        eprintln!("atomos is the H1 binary; use atomos-proto for engine=tokio");
        std::process::exit(1);
    }
    let sock = cfg.control_socket.clone();
    let (router, ctx, _) = atomos::static_router(cfg, rules);
    let ctl = ctx.clone();
    std::thread::spawn(move || {
        let _ = atomos::ops::control_std::serve_control(sock, ctl);
    });
    if let Err(e) = atomos::net::epoll::run(router, ctx) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
```

Export `control_std` from `ops/mod.rs`. `epoll::run` must be `pub` (or `pub use` from lib).

`src/bin/proto.rs`: copy `serve.rs` but `#[tokio::main(flavor = "current_thread")]`, require `engine=tokio` (default if missing), `http2`/`http3` default true **in this binary** by setting fields after load if the user did not pass engine. Call `atomos::serve::run` and tokio `control::serve_control`.

- [ ] **Step 4: Test**

```bash
unset RUSTFLAGS
export CARGO_TARGET_DIR=$HOME/.cache/atomos-target
cargo test --lib
cargo test --test epoll_smoke --test http_smoke --test json_bomb --test rules_dry
cargo test --features proto --test http2_h3
cargo tree -p atomos --no-default-features --features h1 -i quinn ; echo "exit $?"
# expect non-zero from cargo tree
cargo test --features proto --lib
```

Expected: default tests pass; proto tests pass with `--features proto`; `cargo tree -i quinn` without proto fails to find quinn.

Fix `examples/first_app.rs`: gate h2/h3 on `feature = "proto"` or document `cargo run --example first_app --features proto`. Default example stays H1.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml src/bin/serve.rs src/bin/proto.rs src/ops/control_std.rs src/ops/mod.rs src/net/mod.rs src/lib.rs src/net/engine.rs examples tests
git commit -m "feat: TCB split — atomos H1 vs atomos-proto (I_tcb)"
```

---

### Task 4: Supervisor two-generation SIGHUP, drain timeout, heartbeat

**Files:**
- Modify: `src/ops/sup.rs`
- Modify: `src/bin/sup.rs`
- Modify: `src/kernel/config.rs` (`worker_shutdown_timeout_ms`, default 2000)
- Create: `src/ops/heartbeat.rs` (optional; can live in `sup.rs`)
- Test: `src/ops/sup.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: `WorkerSpec { exe, args, n }`
- Produces:
  - `WorkerSpec { exe, args, n, shutdown_timeout: Duration, heartbeat_ms: u64 }`
  - SIGHUP handler: spawn generation `g+1` with same `n`, then `drain` generation `g` using `shutdown_timeout` (SIGTERM then SIGKILL)
  - Heartbeat: each worker writes one byte every `heartbeat_ms` to a pipe inherited from sup; missing 3 intervals → restart that index
  - SIGTERM: drain current generation only (existing behaviour, timeout from spec not hardcoded 2s)

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn shutdown_timeout_from_config() {
    let c = Config::from_json(
        br#"{"bind":"127.0.0.1:0","memory_cap_bytes":67108864,"worker_shutdown_timeout_ms":1500}"#,
    )
    .unwrap();
    assert_eq!(c.worker_shutdown_timeout_ms, 1500);
}

#[test]
fn drain_kills_after_timeout() {
    // spawn `sleep 30`, drain with 200ms, assert process gone
    use std::process::Command;
    use std::time::Duration;
    let child = Command::new("sleep").arg("30").spawn().unwrap();
    let pid = child.id();
    let mut kids = vec![Some(child)];
    super::drain(&mut kids, Duration::from_millis(200));
    assert!(kids.iter().all(|k| k.is_none()));
    let still = Command::new("kill").args(["-0", &pid.to_string()]).status().unwrap();
    assert!(!still.success());
}
```

Generation test: parse a tiny helper — if too heavy for unit, test `next_generation` function:

```rust
#[test]
fn next_generation_increments_and_keeps_n() {
    let g = Generation { id: 3, n: 4 };
    let n = g.next();
    assert_eq!(n.id, 4);
    assert_eq!(n.n, 4);
}
```

- [ ] **Step 2: Run — fail on missing field / `drain` arity**

```bash
cargo test --lib sup::tests::shutdown_timeout_from_config
```

Expected: FAIL (unknown field or missing).

- [ ] **Step 3: Implementation**

`Config`:

```rust
#[serde(default = "default_shutdown_ms")]
pub worker_shutdown_timeout_ms: u64,
fn default_shutdown_ms() -> u64 { 2000 }
```

`sup.rs`:

```rust
static SUP_HUP: AtomicBool = AtomicBool::new(false);
extern "C" fn on_hup(_: libc::c_int) {
    SUP_HUP.store(true, Ordering::SeqCst);
}

pub struct WorkerSpec {
    pub exe: std::path::PathBuf,
    pub args: Vec<String>,
    pub n: u32,
    pub shutdown_timeout: Duration,
}

struct Generation {
    id: u64,
    kids: Vec<Option<Child>>,
}

fn drain(kids: &mut [Option<Child>], timeout: Duration) { /* SIGTERM, wait timeout, SIGKILL; same as today with param */ }

pub fn run(spec: WorkerSpec) -> Result<(), ServeError> {
    // signal SIGTERM/INT as today; additionally SIGUSR1 or SIGHUP → SUP_HUP
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGHUP, on_hup as extern "C" fn(libc::c_int) as *const () as libc::sighandler_t);
    }
    let mut live = spawn_generation(&spec, 0)?;
    loop {
        if SUP_STOP.load(Ordering::SeqCst) {
            drain(&mut live.kids, spec.shutdown_timeout);
            return Ok(());
        }
        if SUP_HUP.swap(false, Ordering::SeqCst) {
            match spawn_generation(&spec, live.id + 1) {
                Ok(new) => {
                    drain(&mut live.kids, spec.shutdown_timeout);
                    live = new;
                }
                Err(e) => tracing::error!(%e, "SIGHUP spawn failed; keeping old generation"),
            }
        }
        std::thread::sleep(Duration::from_millis(200));
        for (i, slot) in live.kids.iter_mut().enumerate() {
            let dead = slot.as_mut().map(|c| c.try_wait().ok().flatten().is_some()).unwrap_or(true);
            if dead {
                *slot = Some(spawn_one(&spec, i as u32)?);
            }
        }
    }
}
```

Heartbeat (minimal unique CC-02): worker already has `ATOMOS_WORKER_INDEX`. Add `ATOMOS_HEARTBEAT_FD` if spec sets it. If the extra fd is too much for this task, document heartbeat as: sup treats **exit** as the heartbeat (already) and add a ctl atom `worker-ping` later. **Do implement** a file-based heartbeat to inhabit the axis:

Worker thread in `epoll::worker` every 5 s: `std::fs::write(runtime_dir.join(format!("atomos-hb-{index}")), b"1")` — **no**, that is not message passing and hits disk (wear). Unique: `libc::write` on an inherited eventfd/pipe.

In `spawn_one`:

```rust
let (r, w) = nix_or_libc_pipe()?;
cmd.env("ATOMOS_HEARTBEAT_FD", w.to_string());
// parent keeps r, nonblocking; loop: if read yields nothing for 3s, restart
```

Without adding `nix` crate: `libc::pipe2`. Put pipe ends in `Generation`.

Workers: in epoll `worker` loop, every N epoll timeouts, `libc::write(hb_fd, &1u64.to_ne_bytes(), 8)`.

- [ ] **Step 4: Test**

```bash
cargo test --lib sup::
cargo test --lib config::tests::shutdown_timeout_from_config
```

Expected: PASS. Do not send SIGHUP to the live PaperRetrieval process.

- [ ] **Step 5: Commit**

```bash
git add src/ops/sup.rs src/bin/sup.rs src/kernel/config.rs src/net/epoll.rs
git commit -m "feat: SIGHUP two-generation reload, drain timeout, heartbeat pipe"
```

---

### Task 5: Jail leftovers — Landlock, seccomp, capset (I_jail)

**Files:**
- Modify: `src/ops/jail.rs`
- Modify: `src/kernel/config.rs` (`landlock: bool` default true on linux; `seccomp: bool` default true; `drop_caps: bool` default true)
- Test: `src/ops/jail.rs`

**Interfaces:**
- Consumes: `jail::after_bind(&Config)` (already)
- Produces: after `NO_NEW_PRIVS` + setuid: `landlock_restrict_self` allowing `static_root`, `rules_path`, `control_socket` parent, `/etc/localtime` optional; seccomp allowlist: `read write close epoll_* accept* recv* send* mmap mprotect brk clone futex nanosleep exit_group rt_sigreturn pipe2 writev clock_gettime getrandom` (extend only with a comment naming the syscall); `capset` empty if `drop_caps`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(target_os = "linux")]
#[test]
fn after_bind_landlock_blocks_etc_passwd_read_when_enabled() {
    // Only if we can create a throwaway tmp static_root.
    // Skip if not linux. This test calls a helper `landlock_ruleset_for(paths)`
    // and then tries Open /etc/passwd — must fail with EACCES/EPERM.
}
```

Prefer a **pure** unit of the allowlist so CI without CAP_SYS_ADMIN still passes:

```rust
#[test]
fn seccomp_allowlist_contains_epoll_wait_and_not_execve() {
    let a = super::SECCOMP_ALLOW;
    assert!(a.contains(&libc::SYS_epoll_wait) || a.contains(&libc::SYS_epoll_pwait));
    assert!(!a.contains(&libc::SYS_execve));
}
```

- [ ] **Step 2: Run fail**

```bash
cargo test --lib jail::tests::seccomp_allowlist_contains_epoll_wait_and_not_execve
```

Expected: FAIL (identifier missing).

- [ ] **Step 3: Implementation**

Keep Landlock/seccomp behind `cfg.landlock` / `cfg.seccomp` so tests that exec helpers are not bricked. Default **true** in production config; **false** in unit tests’ JSON.

Do not add `libseccomp` if a raw BPF program in 80 lines works; if BPF is too error-prone, `libseccomp-sys` is an L-02 TCB add — prefer `seccompiler` or a small `libc::syscall(SYS_seccomp, …)` with a documented filter. If the filter cannot be reviewed in one page, ship Landlock first and seccomp as a second commit in this task.

Landlock (Linux 5.13+): use `libc` constants if present; otherwise raw numbers documented for ABI 3. Fail **open with a warn** if `ENOSYS` (old kernel) — loopback origin still safe; fail **closed** if `cfg.landlock` and errno is not ENOSYS/EOPNOTSUPP.

capset: after setuid, `prctl(PR_CAPBSET_DROP, cap)` for 0..63 or `capset` empty. Root-only path already in `drop_privs`.

- [ ] **Step 4: Test**

```bash
cargo test --lib jail::
```

Expected: PASS on this Linux. `epoll_smoke` still works with `"landlock": false, "seccomp": false` in test JSON so the test process can spawn.

- [ ] **Step 5: Commit**

```bash
git add src/ops/jail.rs src/kernel/config.rs
git commit -m "feat: Landlock, seccomp allowlist, cap-drop after bind (I_jail)"
```

---

### Task 6: `atomos-keyd` (neverbleed analog)

**Files:**
- Create: `src/bin/keyd.rs`
- Create: `src/net/keyclient.rs` under `#[cfg(feature = "proto")]`
- Modify: `src/net/tls.rs` to load public cert in-worker and sign via UDS
- Modify: `Cargo.toml` `[[bin]] atomos-keyd required-features = ["proto"]`
- Test: `tests/keyd_sign.rs` gated `--features proto`

**Interfaces:**
- Consumes: PEM key path argv; socket path `$XDG_RUNTIME_DIR/atomos-keyd.sock` mode 0600, `SO_PEERCRED`
- Produces: length-prefixed request `sign | digest-bytes` → `signature-bytes`. Workers never `include` the private key in their address space after boot. If keyd is down, TLS accept fails closed.

Protocol (fixed, no serde on the datapath):

```
req:  u32be n | u8 kind | [n-1 bytes]
kind 1 = sign_ecdsa_or_rsa (raw digest, rustls SigningKey)
rep:  u32be n | payload
```

- [ ] **Step 1: Failing test**

```rust
#[cfg(feature = "proto")]
#[test]
fn keyd_refuses_other_uid() {
    // bind dummy; peer_euid_ok false → no sign
}
```

And:

```rust
#[tokio::test]
async fn proto_tls_fails_closed_without_keyd() {
    // tls_key points to a path; ATOMOS_KEYD_SOCK missing → ServeError::Config
}
```

Simpler unit:

```rust
#[test]
fn sign_request_roundtrip_bytes() {
    let d = [7u8; 32];
    let b = super::encode_req(1, &d);
    let (k, p) = super::decode_req(&b).unwrap();
    assert_eq!(k, 1);
    assert_eq!(p, d);
}
```

Put encode/decode in `src/net/keyproto.rs` (tiny, no rustls) so H1 can compile it, or keep under proto.

- [ ] **Step 2: Run fail**

```bash
cargo test --features proto --lib keyproto::
```

Expected: FAIL missing module.

- [ ] **Step 3: Implementation**

`keyproto.rs` in `kernel/` or `ops/` (no TLS): encode/decode only.

`keyd.rs`: load PEM with rustls-pemfile; UnixListener; PEERCRED; sign with the rustls `SigningKey` API available in 0.23. Keep the process single-threaded (declared model).

`tls.rs`: if `cfg.tls_key` is `Some` and env `ATOMOS_KEYD_SOCK` is set, do not load the private key in-worker; use a custom `rustls::sign::SigningKey` that writes the digest to the socket. If env unset, **fail config** when `cfg.keyd_required` (new bool, default true when tls_key set). For tests, `keyd_required: false` loads the key in-process (documented waiver for unit tests only).

- [ ] **Step 4: Test**

```bash
cargo test --features proto --lib
```

- [ ] **Step 5: Commit**

```bash
git add src/bin/keyd.rs src/net/keyclient.rs src/ops/keyproto.rs src/net/tls.rs Cargo.toml
git commit -m "feat: atomos-keyd isolates private keys (CC-02)"
```

---

### Task 7: Wasm host off cache-hit (I_cap, I_hit)

**Files:**
- Create: `src/plugin/wasm.rs`
- Modify: `src/plugin/registry.rs` (`PluginKind::Wasm` loads when `feature = "wasm"`)
- Modify: `src/plugin/mod.rs`
- Modify: `Cargo.toml` `wasm = ["dep:wasmtime"]` with `wasmtime` optional, component-model feature
- Create: `plugins/example/fuel_bomb.rs` or a tiny `.wat` component in `tests/fixtures/module.wat` compiled in the test if the toolchain allows; otherwise mock a `WasmModule` trait in unit tests and one ignored integration `#[ignore]` if wasmtime component build is heavy
- Test: `src/plugin/wasm.rs` + existing `cache_second_hit_skips_module`

**Interfaces:**
- Consumes: `wit/atomos-module.wit`; `PluginManifest { kind: Wasm, path }`
- Produces: `Handler::Sync(Arc<WasmMod>)` where `WasmMod: Module`. `handle` instantiates or calls with **fuel**; epoch interrupt every N ms from a timer thread in the worker process (ops, not on GET if no wasm in the ruleset). `load_dir` maps WIT `handle` → `Out`. Cache path unchanged (never calls `Handler`).

Pin wasmtime to a version that builds on 1.80 if possible; if rust-version must bump, state it in Cargo.toml `rust-version` and the commit message.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn wasm_kind_without_feature_errors() {
    // default build: load_dir wasm json → Config error containing "host not linked" (already have)
}

#[cfg(feature = "wasm")]
#[test]
fn wasm_fuel_exhaustion_is_capacity() {
    // host::call with fuel=1 on a looping component → ServeError::Capacity
}

#[test]
fn cache_hit_does_not_call_handler() {
    // already in lib.rs; keep. Add a counting Module and assert call count == 1 after put+get+dispatch.
}
```

Add counting module test in `src/kernel/route.rs` or `lib.rs`:

```rust
struct CountMod(std::sync::atomic::AtomicU32);
impl Module for CountMod {
    fn name(&self) -> &'static str { "count" }
    fn handle(&self, _req: &In<'_>) -> Result<Out, ServeError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Out::json(Status::OK, bytes::Bytes::from_static(b"{}")))
    }
}
```

Dispatch twice with Global cache: counter is 1.

- [ ] **Step 2: Run**

```bash
cargo test --lib cache_hit_does_not_call_handler
```

May already pass via `cache_second_hit_skips_module`. The wasm fuel test fails without the host.

- [ ] **Step 3: Implementation**

`registry.rs`:

```rust
PluginKind::Wasm => {
    #[cfg(not(feature = "wasm"))]
    { return Err(ServeError::Config("wasm plugin host not linked".into())); }
    #[cfg(feature = "wasm")]
    {
        let p = man.path.as_ref().ok_or(...)?;
        let m = crate::plugin::wasm::load(&dir.join(p))?;
        router.insert(man.name.clone(), Handler::Sync(m));
        loaded.push(man.name);
    }
}
```

`Router::insert` takes `&self` already. `load_dir` currently has `&Router` — good.

Wasm `handle` must copy `In` into WIT `request` (owned lists). Bound: `body.len() ≤ max_body_bytes` already. Fuel: `cfg.wasm_fuel` default 10_000_000. Epoch: `Engine::increment_epoch` from a 10 ms tick thread started once per process in `wasm::init()`.

**Never** call wasm from `ResponseCache::get`. No change to `route.rs` dispatch order.

- [ ] **Step 4: Test**

```bash
cargo test --lib
cargo test --features wasm --lib
```

- [ ] **Step 5: Commit**

```bash
git add src/plugin Cargo.toml wit/atomos-module.wit
git commit -m "feat: wasmtime host with fuel/epoch; cache hit still skips plugins"
```

---

### Task 8: Proto tickets/OCSP, metrics/log, smuggling, docs occupancy

**Files:**
- Modify: `src/net/tls.rs` — ticket key `ArcSwap<[u8; 32]>`, rotate atom; OCSP file `tls_ocsp` path stapled
- Create: `src/kernel/metrics.rs` — `LineAtomicU64` counters: `hits`, `misses`, `requests`, `bytes_out`
- Create: `src/kernel/metrics_mod.rs` or `src/ops/metrics_atom.rs` — Module `metrics` renders Prometheus text from atomics (pure snapshot)
- Create: `src/net/access_log.rs` — after successful encode, if `out.flags` contains `FLAG_LOG` or config `access_log: true`, write one line to fd (bounded, no `format!` on hot path if possible; `itoa`)
- Modify: `src/kernel/route.rs` — increment metrics; **not** on a mutex; atomics only
- Create: `tests/smuggling.rs`
- Modify: `docs/architecture.md` (five processes, CC-00 models)
- Modify: `docs/planes.md` `docs/lack.md` `docs/scorecard.md` (recount have/partial/forfeit **only** for items this plan actually landed)
- Modify: `docs/ctl.md` `docs/config.md` for new fields

**Interfaces:**
- Consumes: `FlagSet`, `Out`, atomics
- Produces: `GET /metrics` via a registered module (consumer or example); access log effect **after** write; smuggling → 400

- [ ] **Step 1: Failing tests**

`tests/smuggling.rs`:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn content_length_and_transfer_encoding_rejected() {
    // bind engine=epoll, send:
    // POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\nWAIT
    // expect 400
}
```

`src/kernel/metrics.rs`:

```rust
#[test]
fn snapshot_is_pure() {
    let m = Metrics::new();
    m.requests.fetch_add(1, Ordering::Relaxed);
    let a = m.snapshot();
    let b = m.snapshot();
    assert_eq!(a.requests, b.requests);
}
```

Parse: if both CL and TE present, `ParseStatus::Error` / 400. Add unit test in `src/net/parse.rs`.

- [ ] **Step 2: Run fail**

```bash
cargo test --test smuggling
cargo test --lib parse::tests::cl_plus_te_is_error
```

- [ ] **Step 3: Implementation**

`parse.rs`: after headers, if `content-length` present **and** `transfer-encoding` present → error. That is the unique smuggling inhabitant (RFC 9112).

Metrics: `#[repr(C, align(64))]` atomics in `kernel/align.rs` style. `route.rs` `dispatch`: on hit, `metrics.hits += 1`; on miss after module, `misses += 1`. Cache hit still no module.

Access log: `net/epoll.rs` after `write`, if `cfg.access_log` { write one line }. Not inside `handle`.

TLS tickets/OCSP: only `#[cfg(feature = "proto")]`. `Config.tls_ocsp: Option<PathBuf>`. Rotate: atom `ticket_epoch` plus `ArcSwap` of keys; ctl atom `tls-rotate` in ops (effectful).

H2 cache-hit on connection task: in `h2serve.rs`, on cache hit, write DATA on the existing stream future without `tokio::spawn` for that response. Read the current spawn sites and remove spawn for the hit arm only.

- [ ] **Step 4: Test**

```bash
unset RUSTFLAGS
export CARGO_TARGET_DIR=$HOME/.cache/atomos-target
cargo test --lib
cargo test --test smuggling --test epoll_smoke --test http_smoke
cargo test --features proto --test http2_h3
cargo test --features proto,wasm --lib
cargo clippy --all-targets -- -D warnings
cargo clippy --features proto --all-targets -- -D warnings
```

Expected: PASS. Update scorecard numbers from the spec **only for landed items**. If Task 6/7 were skipped, do not mark them have.

- [ ] **Step 5: Commit**

```bash
git add src tests docs
git commit -m "feat: metrics, access log, smuggling tests, proto tickets/OCSP; scorecard S occupancy"
```

---

## H-04 items explicitly not in this plan

Do not implement unless a **new** matrix in `docs/h04-matrix.md` shows gain ≫ `T_sys`:

- Rust SIMD pico-class parser
- `writev` / `sendfile` / `MSG_ZEROCOPY`
- `EPOLLEXCLUSIVE`
- `recvmmsg`
- Reopening io_uring, prefetch, simd-json
- `.so` / Lua (gain theorem: no)
- kTLS, extra TLS stacks, userspace TCP, XDP on loopback

---

## Self-review (spec coverage)

| Spec § | Task |
|---|---|
| 3.2 coproduct of processes / CC-00 | 1, 2, 3 |
| 4.2 cache hit | 7 (preserve), 8 (metrics must not break I_hit) |
| 4.3 / 5 refuse `.so` Lua | already have; 7 Wasm |
| 6 TCB split | 3 |
| 7.1 SIGHUP drain heartbeat | 4 |
| 7.1 jail | 5 |
| 7.1 keyd tickets OCSP | 6, 8 |
| 7.1 metrics log fuzz/smuggling | 8 (fuzz harness: cargo-fuzz optional; smuggling required) |
| 7.3 forfeits | not implemented (correct) |
| 8 projections | docs in Task 8 |
| 10 bind 8082 | all tests |

Privilege-separated config parse: folded into Task 4 (`atomos-sup` can `Config::load_path` and pass `--config` to children — already argv). No extra parser process unless a later spec.

Placeholder scan: no TBD/TODO/implement-later.

Type names: `EngineKind::{Epoll,Tokio,Xdp}`, `AtomCtx` without `wake`, `WorkerSpec.shutdown_timeout`, `Metrics.snapshot`, `control_std::serve_control`.
