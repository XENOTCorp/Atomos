# Modules and rules

Each module has a name. The ruleset maps `(method, path)` to **one** module.
Overlap at load is `RuleError::Overlap`. No regex. Patterns are exact (`/x`) or
prefix (`/api/*`). `exclude` punches holes (`/*` except `/api/*`).

## Pipeline flags on `Out`

Examples of fields a module may set (see `src/io.rs`):

- `status` — `Status::OK` or `Status::NOT_FOUND`
- `reason` — None → RFC phrase
- `headers` — Content-Type, ETag, Location
- `body` — Raw (files) or Json (already serialized)
- `cache` — No (default) | Global `{ ttl_ms }` | Named `{ id, ttl_ms }`
- `flags` — `FLAG_LOG`, `FLAG_METRICS_SKIP`, `FLAG_NO_POST`, `FLAG_DEGRADED`

## Pre / post

Config keys `pre_module` and `post_module` name RAM-resident modules. The
default `static_router` leaves them unset. After `static_router`, insert
handlers and set `router.pre` / `router.post`. Pre runs before the ruleset;
status ≥ 400 short-circuits. Post may rewrite body/headers.

Header rules on a rule: `{ "name":"key", "exists": true, "on_fail": 401 }`.
CIDR is currently only `127.0.0.0/8` (loopback check).

## Refresh

`rules.reload` reads `rules_path` and swaps the `Arc`. Compiled `.rs` is not
hot-loaded. Add a module by shipping code, add a **path** by editing JSON.

## Dry test

Atom `rules.dry_test` or control command `dry-test-rules`. Two `/*` GET rules
without exclude → `{ ok: false, a, b, example_path }`.
