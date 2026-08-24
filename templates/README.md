# Templates

Copy these into a **consumer** crate. They are not compiled as part of `atomos`.

| File | Kind |
|---|---|
| `atom_pure.rs` | Read-only atom (`signal.get`, `resource.get` style) |
| `atom_effectful.rs` | Write atom (`json.crud`, `server.stop` style) |
| `molecule.rs` | Named list of atom names |
| `module_sync.rs` | `Module` for static / CPU-light paths |
| `module_async.rs` | `AsyncModule` for `/api/*` |
| `module_pre.rs` | Optional pre-ruleset hook |
| `module_post.rs` | Optional post-module hook |
| `config.json` | Kernel JSON |
| `rules.json` | Disjoint include/exclude + optional header rule |
| `error.html` | `{{code}}` `{{phrase}}` `{{detail}}` |

Hot-reload of `.rs` is not supported. Reload **JSON** rules with the control
command `refresh-endpoints` (`rules.reload` atom).

## CPU / memory guidance for endpoint authors

The H1 datapath (atomos epoll on fds-core) is per-core, run-to-completion,
**zero-allocation per request** (receive buffers, the connection table and
the encoder scratch are preallocated at startup). Two things therefore do
*not* speed up the hot loop, but are documented for control-path needs:

- **jemalloc / mimalloc** — swap the global allocator in your *binary*
  crate when control-path allocation patterns matter (large responses,
  connection setup). One `#[global_allocator]` per process; see the
  commented pattern in `module_sync.rs`.
- **Lock-free handoff** — endpoints that talk to background threads
  (fan-out, logs, writer pools) should use `crossbeam-channel` (bounded),
  `tokio::sync::mpsc` (async endpoints), or atomics + a fixed ring.
  `parking_lot` mutexes are fine on the control path; never take a lock
  inside `handle()` — a blocked handler stalls the whole worker.
  See the per-file comments in `module_{sync,async,pre,post}.rs`.

Worked first app: [docs/first-web-app.md](../docs/first-web-app.md) and
`examples/first_app.rs`.
