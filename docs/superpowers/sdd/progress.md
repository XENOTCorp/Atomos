# SDD ledger — plan: docs/superpowers/plans/2026-08-20-maximal-origin.md

Ruling: User asked for many parallel agents in ~/Projects/Atomos. Exclusive files; separate CARGO_TARGET_DIR per agent. No git commit. No 8082.

| Agent | Files | Status |
|---|---|---|
| jail | src/ops/jail.rs | running 01a02245-19c9-7c73-8715-6b8349a1b888 |
| metrics | kernel/metrics.rs net/access_log.rs route.rs lib.rs epoll.rs | running 01a02245-19c9-7c73-8715-6ba68bedbef7 |
| wasm | plugin/wasm.rs registry.rs | running 01a02245-19c9-7c73-8715-6bb567a3c985 |
| keyd | bin/keyd.rs ops/keyproto.rs | running 01a02245-19c9-7c73-8715-6bc9b8b5a0d3 |
| tls | net/tls.rs | running 01a02245-19c9-7c73-8715-6bd91f6ada0d |

Proto cargo feature TCB split: orchestrator after agents (touches too many files).
