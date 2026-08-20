# ATOMOS

Extractable HTTP kernel: disjoint JSON rules, modules (`In` → `Out`),
pure/effectful atoms, optional operator TUI. No axum, no regex router, no
reqwest.

Default listen: **127.0.0.1:8090**. This crate does not bind 8082.

## Quick start

```bash
unset RUSTFLAGS
export CARGO_TARGET_DIR=$HOME/.cache/atomos-target   # /tmp is noexec on XENOT
cargo run --release --example static_site -- examples/static 127.0.0.1:8090
```

Operator TUI (other terminal):

```bash
cargo run --release --bin atomos-ctl -- --config examples/config.json --json examples/data.json
# optional: cargo run --release --bin atomos-ctl -- install-link   # ~/atomos
```

Consumer crate:

```toml
atomos = { path = "../Atomos" }
```

Register a `Module`, parse `Ruleset`, call `atomos::serve::run`.

## Layout

| Path | What |
|---|---|
| `src/` | kernel |
| `src/tui.rs` | operator dashboard (feature `tui`) |
| `src/bin/serve.rs` | `atomos` static server |
| `src/bin/ctl.rs` | `atomos-ctl` |
| `examples/` | `static_site`, `echo_api`, sample config/rules/data |
| `templates/` | copy-paste atoms, molecules, modules, JSON |
| `docs/` | architecture, atoms, modules, TUI, performance, config |

## Guides

1. [Architecture](docs/architecture.md)
2. [Atoms and molecules](docs/atoms.md)
3. [Modules and rules](docs/modules.md)
4. [TUI](docs/tui.md)
5. [Performance](docs/performance.md)
6. [Config](docs/config.md)
7. [Templates](templates/README.md)

## Build

lld + RELRO/now/noexecstack via `.cargo/config.toml`. Release: opt-3, thin LTO,
codegen-units 1, panic abort, strip. Warnings are errors on the library.
`cargo clippy --all-targets -- -D warnings`.

Warnings-as-errors and ARCSS (`~/arcss.txt`) apply to this crate the same way
as the original kernel extract.
