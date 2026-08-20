# Architecture

ATOMOS is a barebones HTTP kernel. A **consumer** crate registers named modules
and loads a disjoint JSON ruleset. The kernel does not know about any product.

```
accept (socket2: SO_REUSEPORT, TCP_NODELAY, optional TFO)
  → optional pre module
  → httparse on the receive buffer
  → first-byte + depth/size scan of JSON bodies
  → ruleset match (RAM, arc-swap)
  → module(In) → Out { status, headers, body, cache, flags }
  → optional post module (sees flags)
  → optional response cache
  → write (itoa for status and Content-Length)
```

Tokio workers accept / parse / write. Blocking CPU belongs in the consumer
(`spawn_blocking` / rayon). Upstream HTTP belongs in a bounded queue atom,
never on the request path.

## Types

- `In<'buf>` — path, query, headers, body borrow the receive buffer (or a
  request-scoped bumpalo copy after parse).
- `Out` — owned response. Cache default is off.
- `Module` / `AsyncModule` — one name, one handler.
- `Atom` — pure or effectful JSON in/out. The TUI talks only through atoms.
- `Molecule` — named list of atom names (restart = stop then start).

## What this crate is not

- Not axum / regex routers / reqwest.
- Not a product (no search index, no scholarly APIs).
- Default bind is **127.0.0.1:8090**, never 8082 (HYPERSCIRET).
- io_uring, QUIC, simd-json, `.so` reload: measured, not shipped. See
  `docs/performance.md`.
