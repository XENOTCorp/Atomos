# Brief: wasmtime host off cache-hit

Repo: `/home/xenot/Projects/Atomos`
Cargo already has `wasmtime` optional, feature `wasm = ["dep:wasmtime"]`.
WIT: `wit/atomos-module.wit` (package atomos:module@0.1.0, world module, handle request -> result<response, string>).

You MAY create `src/plugin/wasm.rs`
You MAY edit `src/plugin/mod.rs`, `src/plugin/registry.rs`
Do NOT edit Cargo.toml, config.rs (wasm_fuel already on Config, default 10_000_000), route.rs, jail.rs, tls.rs, bins.
Do NOT commit. Do NOT bind 8082.

CARGO_TARGET_DIR=$HOME/.cache/atomos-target ; unset RUSTFLAGS

## Behavior
- `#[cfg(feature = "wasm")]` `pub fn load(path: &Path, fuel: u64) -> Result<Arc<dyn Module>, ServeError>`
- Implement `Module`: copy In into WIT request (owned), call handle with fuel store, map response to Out (status, headers, body, cache Global if cache-ttl-ms>0 else No)
- Fuel exhaustion → ServeError::Capacity
- Epoch interrupt: Engine::epoch_interruption + a thread or `increment_epoch` every 10ms while any instance runs is OK; keep it simple
- registry `PluginKind::Wasm`:
  - without feature: existing "host not linked" error
  - with feature: load wasm next to manifest (`man.path` relative to json file), `router.insert(name, Handler::Sync(...))`

## Tests
Keep `native_so_is_refused`.
`#[cfg(not(feature = "wasm"))] wasm_kind_without_feature_errors`: load_dir wasm json → Config error containing "host" or "wasm".
`#[cfg(feature = "wasm")]` if you can ship a tiny component fixture, fuel test; if WIT component compile is too heavy, unit-test a `fuel_to_capacity` mapping with a mock and document BLOCKED fixture.

Never call wasm from cache get. Do not change route.rs dispatch order.

If wasmtime 24 fails to compile on this rustc, try 22 or 25; do not bump rust-version above 1.80 unless required; if required, set rust-version in a comment in the report only: do not edit Cargo.toml rust-version (orchestrator will).

Report: `/home/xenot/Projects/Atomos/docs/superpowers/sdd/reports/wasm.md`
No subagents. No git commit.
Run: `cargo test --lib plugin::` and `cargo test --features wasm --lib plugin::`
