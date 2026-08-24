# Architecture

ATOMOS is a barebones HTTP kernel. A **consumer** crate registers named modules
and loads a disjoint JSON ruleset. The kernel does not know about any product.

```
atomos (H1): N pinned OS threads, one FDS reactor each, Conn=slot, no spawn
  Built on the FDS transport engine (fds-core): edge-triggered epoll
  with drain-to-EAGAIN, FDS TCP transport (options before bind for
  SO_REUSEPORT group admission), FDS preallocated ConnTable with
  hot/cold cache-line halves and packed ConnectionId tokens.
  TCP SO_REUSEPORT HTTP/1.1 only
atomos-proto (optional process): pinned current-thread tokio
  TCP peek → TLS | HTTP/2 preface | HTTP/1.1; UDP HTTP/3
shared kernel:
    → optional pre module
    → parse (httparse / h2 / h3)
    → first-byte + depth/size scan of JSON bodies
    → ruleset match (RAM, arc-swap)
    → module(In) → Out { status, headers, body, cache, flags }
    → optional post module (sees flags)
    → optional response cache
    → encode: HTTP/1.1 wire | HTTP/2 DATA | HTTP/3 DATA
```

Four planes: [planes.md](planes.md). Gap vs nginx/h2o/Pingora: [lack.md](lack.md).

Pinned workers accept / parse / write on their own core. Blocking CPU belongs
in the consumer (`spawn_blocking` / rayon). Upstream HTTP belongs in a bounded
queue atom, never on the request path. HTTP/1.1 keep-alive uses the encoded
byte cache; HTTP/2 and HTTP/3 use the semantic `Out` cache (same epoch).

## Types

- `In<'buf>` — path, query, headers, body borrow the receive buffer.
- `Out` — owned response. Cache default is off.
- `Module` / `AsyncModule` — one name, one handler.
- `Atom` — pure or effectful JSON in/out. The operator ctl talks only through atoms.
- `Molecule` — named list of atom names (restart = stop then start).

## What this crate is not

- Not axum / regex routers / reqwest.
- Not a product (no search index, no scholarly APIs).
- Default bind is **127.0.0.1:8090**. Ports to skip are `refuse_ports` /
  `scripts/atomos-host.sh` (kernel has no hardcoded port list).
- The H1 engine's reactor/sockets/conn-table come from **FDS**
  (`fds-core`, sibling repo `~/Projects/FDS`); the HTTP kernel (parse,
  route, cache, encode, jail) is Atomos's own.
- io_uring, simd-json, `.so` reload: measured, not shipped. See
  `docs/performance.md`.
- HTTP/2 and HTTP/3 are **off** on `atomos` (epoll). Use `atomos-proto`
  (`engine=tokio`, `http2`/`http3` true). Mixing epoll with h2/h3 is a config error.
