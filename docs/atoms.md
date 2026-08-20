# Atoms and molecules

Atoms are the only mutation API. Kind is `Pure` or `Effectful`. A pure atom
that writes the world is a defect.

## Built-in atoms

| Name | Kind | Input | Output |
|---|---|---|---|
| `signal.get` | pure | `{}` | `{ "state": "on"\|"off"\|"restarting" }` |
| `json.pretty` | pure | any JSON | `{ "text": "…" }` (cap 1 MiB) |
| `resource.get` | pure | `{}` | `{ "rss_bytes", "cpu_fraction", "uptime_ms" }` |
| `rules.dry_test` | pure | `{path}` or `{rules:[…]}` or `{}` | `{ok}` or overlap |
| `json.crud` | effectful | `{path, op, pointer, value?}` | `{ok}` |
| `settings.backup` | effectful | `{path, dest}` | `{ok, bytes}` |
| `server.start` / `stop` / `restart` | effectful | `{}` | signal JSON |
| `rules.reload` | effectful | `{}` | reloads `ctx.rules_path` into `arc-swap` |
| `tunnel.apply` | effectful | `{}` | `{ok:false, error:"unconfigured"}` |

`json.crud` ops: `add`, `put`, `del`. Pointers are JSON Pointer (`/keys/-`
appends). Files > 8 MiB → `AtomError::Bound`. Writes are tmp + rename.

`server.*` set cache-line-aligned atomics. They do **not** spawn a process.
`allow_write: false` on `AtomCtx` turns effectful atoms into `PureActuate`.

## Molecules

```
server.restart  = ["server.stop", "server.start"]
tui.dashboard   = ["signal.get", "resource.get"]
```

Add more in the consumer (`templates/molecule.rs`).

## Control socket

Unix datagram-style JSON lines, mode 0600. Commands: `status`,
`refresh-endpoints` / `rules.reload`, `stop`, `start`, `restart`,
`dry-test-rules`. Default path `/tmp/atomos.sock`.
