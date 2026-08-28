# Atomos

Atomos is an HTTP kernel in Rust. A consumer registers named modules.
A disjoint JSON ruleset maps `(method, path)` to one module.

Requirements:

- Linux
- Rust 1.97.1 or later

Build and test from `Code/`. FDS crates are vendored in `Code/FDS`.

```
cd Code
cargo test
```

Start here: [Docs/Getting-Started.md](Docs/Getting-Started.md).

Wiki: [Docs/Wiki/Home.md](Docs/Wiki/Home.md).

License: MIT. Copyright (c) 2026 XENOT Corporation.
