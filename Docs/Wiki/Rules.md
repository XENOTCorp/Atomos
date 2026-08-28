# Rules

The ruleset maps `(method, path)` to one module. Overlap at load time is an error. There is no regex.

Patterns:

- exact: `/x`
- prefix: `/api/*`

`exclude` punches holes. Example: `/*` except `/api/*`.

```json
{
  "rules": [
    { "id": "static", "module": "static", "methods": ["GET", "HEAD"],
      "include": ["/*"], "exclude": ["/api/*"] },
    { "id": "api", "module": "api", "methods": ["GET", "POST"],
      "include": ["/api/*"], "exclude": [] }
  ]
}
```

JSON fields: `id`, `module`, `methods`, `include`, `exclude`, `headers`.

The matcher is a linear scan for a small ruleset. It is a path trie for a large ruleset. The kernel picks at load time. Both give the same result.

Header rule example: `{ "name": "authorization", "exists": true, "on_fail": 401 }`.

`rules.reload` reads the rules path and swaps the `Arc`. Rust modules are compiled in. They are not hot-loaded. Adding a path is editing JSON. Adding a module is shipping code.
