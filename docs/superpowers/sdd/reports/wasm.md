# Report: wasm

## status

done

Wasmtime 24.0.13 compiled on rustc 1.97.1 (crate `rust-version` left at 1.80). Feature `wasm` links a component host for `wit/atomos-module.wit`. Cache-hit dispatch in `route.rs` is unchanged (still returns before module lookup).

## files changed

- `src/plugin/wasm.rs` (created)
- `src/plugin/mod.rs`
- `src/plugin/registry.rs`
- `docs/superpowers/sdd/reports/wasm.md` (this file)

Not edited: Cargo.toml, config.rs, route.rs, jail.rs, tls.rs, bins.

## tests run + result

```
CARGO_TARGET_DIR=$HOME/.cache/atomos-target-wasm
unset RUSTFLAGS
cargo test --lib plugin::
cargo test --features wasm --lib plugin::
```

- `cargo test --lib plugin::` — **ok**. 3 passed (parses_wasm_manifest, native_so_is_refused, wasm_kind_without_feature_errors).
- `cargo test --features wasm --lib plugin::` — **ok**. 7 passed, 1 ignored (native_so_is_refused, wasm_kind_missing_component_is_config, fuel_to_capacity_{out_of_fuel,interrupt,other_trap}, load_missing_wasm_is_config; ignored wasm_fuel_exhaustion_is_capacity).

## concerns

- **BLOCKED fixture:** no `wasm-tools` / `cargo-component` here, and wasmtime is built without `wat`. A looping WIT component was not compiled. Fuel/epoch → `ServeError::Capacity` is unit-tested via `fuel_to_capacity` on `Trap::OutOfFuel` / `Trap::Interrupt`. Live `handle` with fuel=1 on a looping guest is untested.
- Per-request `instantiate` + fuel store (simple; not a reused instance).
- Epoch tick is 10ms while any instance is live; deadline is 1000 ticks (~10s) so fuel is the usual limiter.
- `Module::name()` is always `"wasm"`; the router key is the manifest name from `insert`.
- `OnceLock::get_or_try_init` is unstable on this rustc (`once_cell_try`); engine init uses `OnceLock::set` instead.
- Environment emitted `avx10.1` / `avx10.2` target-feature warnings (not from this change; `deny(warnings)` did not fail).
