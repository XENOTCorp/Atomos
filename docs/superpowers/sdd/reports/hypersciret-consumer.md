# Report: HYPERSCIRET (PaperRetrieval) → Atomos consumer mapping

**Status:** analysis only (no PaperRetrieval / Atomos src edits)  
**Date:** 2026-08-21  
**Constraint:** Atomos must never bind **8082** (HYPERSCIRET’s loopback HTTP). Atomos default bind is **8090**.

## Molecule tests (Atomos)

```bash
cd /home/xenot/Projects/Atomos
CARGO_TARGET_DIR=$HOME/.cache/atomos-target cargo test --lib molecule::
```

**Result:** 4 passed, 0 failed.

| Test | Outcome |
|------|---------|
| `hybrid_effect_before_pure_is_err` | ok: `MoleculeKind::Hybrid` rejects Effectful-before-Pure |
| `hybrid_pure_then_effect_is_ok` | ok |
| `restart_is_effectful` | ok: `MoleculeKind::Effectful` |
| `ops_dashboard_is_pure` | ok: `MoleculeKind::Pure` |

Confirmed: `MoleculeKind::{Pure, Effectful, Hybrid}` exists; hybrid with effect before pure fails load-time validation.

## Current dependency

HYPERSCIRET (`/home/xenot/Projects/PaperRetrieval`) depends on **`crates/xenot-serve`**, not Atomos:

```toml
xenot-serve = { path = "crates/xenot-serve" }
```

Consumer call sites: `src/http.rs`, `src/main.rs` (also `src/ctl.rs` for `AtomCtx` / `AtomError`).

## Symbol mapping (`xenot_serve::X` → `atomos::Y`)

Atomos re-exports kernel modules at crate root and `net::serve` / `ops::control` at crate root (`Atomos/src/lib.rs`). Import-path rename for the symbols HYPERSCIRET uses:

| HYPERSCIRET today | Atomos equivalent | Notes |
|-------------------|-------------------|-------|
| `xenot_serve::io` | `atomos::io` | `Body`, `In`, `InOwned`, `Method`, `Out`, `HeaderView` |
| `xenot_serve::module` | `atomos::module` | `AsyncModule`, `BoxFut`, `Handler` |
| `xenot_serve::route` | `atomos::route` | `Router`: **struct layout differs** (below) |
| `xenot_serve::rules` | `atomos::rules` | `Ruleset` |
| `xenot_serve::static_mod` | `atomos::static_mod` | `StaticMod` |
| `xenot_serve::status` | `atomos::status` | `Status` |
| `xenot_serve::error_page` | `atomos::error_page` | `ErrorPage` (`builtin` / `load` present) |
| `xenot_serve::error` | `atomos::error` | `ServeError`, `AtomError` |
| `xenot_serve::atom` | `atomos::atom` | `AtomCtx`: fields match |
| `xenot_serve::align` | `atomos::align` | `LineAtomicU8` |
| `xenot_serve::cache` | `atomos::cache` | `ResponseCache` |
| `xenot_serve::governor` | `atomos::governor` | `Governor::from_config` |
| `xenot_serve::config` | `atomos::config` | `Config::from_json`: **extra fields / defaults differ** |
| `xenot_serve::flags` | `atomos::flags` | `FlagSet` |
| `xenot_serve::serve` | `atomos::serve` (`net::serve`) | `serve::run(router, ctx)`: same arity; Atomos adds `refuse_ports` gate |
| `xenot_serve::control` | `atomos::control` (`ops::control`) | `control::serve_control(path, ctx)`: same async signature; Atomos also prepares socket dir / peer euid |

Also available on Atomos (unused by HYPERSCIRET today): `atomos::molecule`, `metrics`, `engine`, `epoll`, `listen`, `parse`, `json_out`, `jail`, `ctl`, `sup`, `plugin`, …

## Extra Atomos deps a path-dep would pull

`xenot-serve` is lean (tokio + httparse + serde + …). A path dependency on Atomos additionally pulls (from `Atomos/Cargo.toml`):

| Crate | Role |
|-------|------|
| **rustls** / **tokio-rustls** / **rustls-pemfile** | TLS |
| **h2** | HTTP/2 |
| **h3** / **h3-quinn** / **quinn** | HTTP/3 / QUIC |
| **http** | http types |
| **rcgen** | self-signed / cert helpers |
| **tracing-subscriber** | (Atomos bins/examples; still in package deps) |

Optional: `wasmtime` behind feature `wasm` (not default). Default features: `h1` only; proto stack is still compiled into the lib via `net::{h2serve,h3serve,tls}`.

## Signature diffs that matter for cutover

### `AtomCtx`

**Compatible.** Both:

```rust
pub struct AtomCtx {
    pub signal: Arc<LineAtomicU8>,
    pub rules: Arc<ArcSwap<Ruleset>>,
    pub rules_path: PathBuf,
    pub started: Instant,
    pub allow_write: bool,
    pub stop: Arc<LineAtomicU8>,
}
```

`AtomCtx::test()` and `run` / `dispatch` exist on both. HYPERSCIRET’s struct-literal construction in `http::build_router` is fine.

### `Router`

**Not compatible as a mechanical field copy.**

| Field | xenot-serve | Atomos |
|-------|-------------|--------|
| `modules` | `ModuleMap` (owned `HashMap`) | `Arc<ArcSwap<ModuleMap>>` |
| `metrics` | *(absent)* | `Arc<Metrics>` (kernel metrics) |
| other | `cfg`, `rules`, `pre`, `post`, `cache`, `gov`, `errors` | same names |

HYPERSCIRET today (`http.rs`):

```rust
let router = Arc::new(Router {
    cache: …,
    gov: …,
    errors,
    rules: ctx.rules.clone(),
    modules,           // plain HashMap
    pre: None,
    post: None,
    cfg: Arc::new(sc),
    // no metrics
});
```

Must become: wrap modules in `Arc::new(ArcSwap::from_pointee(modules))`, supply `metrics: Arc::new(atomos::metrics::Metrics::new())` (or equivalent). Prefer `Router::insert` / `static_router` + insert API where possible.

### `Config::from_json`

**Same signature:** `fn from_json(raw: &[u8]) -> Result<Self, ServeError>`.

Diffs:

| | xenot-serve | Atomos |
|-|-------------|--------|
| Default bind | `127.0.0.1:8082` | `127.0.0.1:8090` |
| `refuse_ports` | absent | `Vec<u16>` (default empty; enforced in `serve::run` / `epoll::run`) |
| `queue_cap` | present | **removed** |
| Extra fields |: | `cpu_pin`, `http2`, `http3`, `tls_*`, `engine`, `plugin_dir`, jail/landlock/seccomp, `access_log`, `wasm_fuel`, `keyd_sock`, … |
| Default workers | `2` | `available_parallelism()` |
| Default control socket | `/tmp/xenot-serve.sock` | `$XDG_RUNTIME_DIR/atomos.sock` (via `runtime_dir()`) |
| Default `engine` | n/a | `"epoll"` |

HYPERSCIRET’s `serve_config` JSON always sets `bind` explicitly from app config (often `127.0.0.1:8082`). That still validates on Atomos unless `refuse_ports` contains that port. **Cutover must keep HYPERSCIRET on 8082 via its own Config; Atomos defaults / refuse list must not steal or collide with 8082 when running Atomos binaries.**

### `serve::run`

**Same signature:** `pub async fn run(router: Arc<Router>, ctx: Arc<AtomCtx>) -> Result<(), ServeError>`.

Atomos additionally:

- rejects bind if port ∈ `cfg.refuse_ports`;
- installs `StopOnDrop` (sets stop on drop);
- supports TLS / H2 / H3 paths when config enables them.

### `control::serve_control`

**Same signature:** `pub async fn serve_control(path: PathBuf, ctx: Arc<AtomCtx>) -> Result<(), ServeError>`.

Atomos adds `jail::prepare_socket_dir` and peer-euid checks. Sync variant: `atomos::control_std::serve_control` (not used by HYPERSCIRET).

## Mechanical import rename would compile?

**No.**

Why:

1. **`Router` layout**: HYPERSCIRET constructs `Router { modules: ModuleMap, … }` without `metrics`. Atomos requires `modules: Arc<ArcSwap<ModuleMap>>` and `metrics: Arc<Metrics>`. Struct literal fails to compile after rename alone.
2. Secondary (runtime / policy, not the first compile break): Atomos `Config` default bind **8090** and optional **`refuse_ports`**; path-dep pulls rustls/h2/h3/quinn. App JSON that omits `engine` gets `engine=epoll` under Atomos (harmless if calling `serve::run` directly, but differs from xenot-serve’s sole tokio accept loop).

A rename-only change of `xenot_serve` → `atomos` in imports is therefore insufficient; `build_router` (and any other `Router { … }` sites) must be adapted.

## Bind / ports policy

| Process | Bind |
|---------|------|
| HYPERSCIRET / paper-retrieval | **8082** (app default; cloudflared edge) |
| Atomos (`atomos`, `atomos-proto`, …) | **8090** default; **must never bind 8082** |

Use Atomos `refuse_ports` (config or host overlay) to hard-fail accidental 8082 binds in Atomos processes. Do not change live HYPERSCIRET bind as part of an Atomos-only cutover experiment.

## Recommended cutover steps (LAST: after HYPERSCIRET tests green)

Do **not** start these until PaperRetrieval’s own test suite is green on xenot-serve.

1. Keep HYPERSCIRET on `crates/xenot-serve` until router adaptation is ready; do not flip `Cargo.toml` early.
2. In a branch, add Atomos as a path dep **alongside** or replacing xenot-serve only after `build_router` is rewritten for `Arc<ArcSwap<ModuleMap>>` + `metrics`.
3. Map imports per table above; fix `Router` construction; keep explicit `"bind":"127.0.0.1:8082"` in HYPERSCIRET serve JSON (Atomos default 8090 must not silently apply).
4. Confirm Atomos host/bin configs use **8090** and set `refuse_ports` to include **8082** for Atomos processes only.
5. Run PaperRetrieval tests + a loopback smoke (`/papers/health`) on 8082 without stopping unrelated PIDs or overwriting `/usr/local/bin/paper-retrieval`.
6. Drop `crates/xenot-serve` from the workspace when smoke + tests pass; document dep weight (rustls/h2/h3/quinn).
7. Commit only when the consumer owner requests it (this analysis did not commit).
