# Atomos

Atomos is an HTTP kernel in Rust. A consumer registers named modules.
A disjoint JSON ruleset maps `(method, path)` to one module.

Linux. Rust 1.97.1 or later.

## Build

```
./compile.sh
```

`compile.sh` reads this machine. It writes CPU flags and the host overlay.
Then it builds the release binaries under `Code/target/release/`.

Do not copy `Code/.cargo/config.toml` or `.atomos/host.json` to another machine.
Run `./compile.sh` on each host.

## Test

```
cd Code
cargo test
```

## Start

[Docs/Getting-Started.md](Docs/Getting-Started.md)

Architecture (planes, pre/post, hot-swap): [Docs/Wiki/Architecture.md](Docs/Wiki/Architecture.md)

Wiki: [Docs/Wiki/Home.md](Docs/Wiki/Home.md)

License: MIT. Copyright (c) 2026 XENOT Corporation.
