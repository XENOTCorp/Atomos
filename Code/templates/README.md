# Templates

Copy these files into a consumer crate. The Atomos package does not compile them.

| File | Kind |
|---|---|
| `atom_pure.rs` | Read-only atom (`signal.get`, `resource.get` style) |
| `atom_effectful.rs` | Write atom (`json.crud`, `server.stop` style) |
| `molecule.rs` | Named list of atom names |
| `module_sync.rs` | `Module` for static and CPU-light paths |
| `module_async.rs` | `AsyncModule` for `/api/*` |
| `module_pre.rs` | Optional pre-ruleset hook |
| `module_post.rs` | Optional post-module hook |
| `config.json` | Kernel JSON |
| `rules.json` | Disjoint include/exclude and optional header rule |
| `error.html` | `{{code}}` `{{phrase}}` `{{detail}}` |

Hot-reload of `.rs` is not supported. Reload JSON rules with `refresh-endpoints` (`rules.reload` atom). See [Docs/Wiki/Architecture.md](../../Docs/Wiki/Architecture.md).

Worked first app: [Docs/Wiki/Examples.md](../../Docs/Wiki/Examples.md) and `examples/first_app.rs`.
