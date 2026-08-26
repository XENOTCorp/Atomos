# Atomos wiki

## Table of contents

1. [Overview](#overview)
2. [Architecture](#architecture)
3. [Requests, responses, and caching](#requests-responses-and-caching)
4. [Rules and routing](#rules-and-routing)
5. [Modules](#modules)
6. [Atoms and molecules](#atoms-and-molecules)
7. [Operator control](#operator-control)
8. [HTTP/2 and HTTP/3](#http2-and-http3)
9. [Planes and plugins](#planes-and-plugins)
10. [Configuration](#configuration)
11. [Performance and bounds](#performance-and-bounds)
12. [Implementation examples](#implementation-examples)
13. [Limitations](#limitations)

## Overview

Atomos is an HTTP kernel in Rust. A consumer registers named modules
and loads a disjoint JSON ruleset. The kernel accepts TCP, parses
headers, scans JSON depth, matches the ruleset, runs the module, and
writes the response.

The kernel does not know about any product. It is not a framework and
not a server binary. It provides the request path; the application
provides the modules.

Two engines exist:

- `atomos` serves HTTP/1.1 on the FDS epoll transport: pinned OS
  threads, one FDS reactor each, preallocated connection table.
- `atomos-proto` serves HTTP/2 and HTTP/3 on tokio: TCP with TLS
  detection, HTTP/2 prior knowledge, QUIC with HTTP/3.

## Architecture

```
atomos (H1): pinned OS threads, one FDS reactor each, conn = slot
  TCP SO_REUSEPORT, HTTP/1.1 only

atomos-proto: pinned tokio workers
  TCP peek, then TLS | HTTP/2 preface | HTTP/1.1; UDP HTTP/3

shared kernel:
  optional pre module
  parse (httparse / h2 / h3)
  first-byte and depth/size scan of JSON bodies
  ruleset match (RAM, arc-swap)
  module(In) -> Out { status, headers, body, cache, flags }
  optional post module (sees flags)
  optional response cache
  encode: HTTP/1.1 wire | HTTP/2 DATA | HTTP/3 DATA
```

Pinned workers accept, parse, and write on their own core. Blocking
work belongs in the consumer (`spawn_blocking` or a worker pool), never
on the request path. HTTP/1.1 keep-alive uses the encoded byte cache;
HTTP/2 and HTTP/3 use the semantic `Out` cache with the same epoch.

The H1 engine runs one FDS `Reactor` per pinned worker, binds FDS
`TcpListener`s with SO_REUSEPORT and options before bind, stores
per-connection HTTP state keyed by FDS `ConnectionId` tokens, and ports
the read/parse/dispatch/wire-cache state machine onto the
drain-to-EAGAIN discipline.

## Requests, responses, and caching

`In<'buf>` borrows the receive buffer: method, path, query, headers,
body, peer, flags. `Out` owns the response: status, reason, headers,
body, cache directive, flags.

Body types: `Empty`, `Raw(Bytes)`, `Json(Bytes)`, `Stream`, `File`.
The H1 epoll path sends `File` with `sendfile`; the tokio paths
materialize it in memory (H2/H3 framing and TLS need the bytes).

Cache directives:

- `No` (default): the response is never cached.
- `Global { ttl_ms }`: cached per worker, keyed by the wire form.
- `Named { ruleset, ttl_ms }`: cached per named set; invalidation by
  name.

The response cache is per worker and preallocated (entries and bytes
are bounded in config). A cached response is served with a single
`writev` and zero per-request syscalls beyond the transport.

## Rules and routing

The ruleset maps `(method, path)` to one module. Overlap at load time
is an error. There is no regex. Patterns are exact (`/x`) or prefix
(`/api/*`). `exclude` punches holes (`/*` except `/api/*`).

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

The matcher is a deterministic automaton for large rule sets and a
linear scan for small ones; the choice is a measured crossover
(16 rules on the reference machine). Both realizations are
allocation-free and produce identical results, verified by a 200,000
path fuzz test.

Header rules on a rule: `{ "name": "authorization", "exists": true,
"on_fail": 401 }`.

`rules.reload` reads the rules path and swaps the `Arc`. Rust modules
are compiled in and are not hot-loaded. Adding a path is editing JSON;
adding a module is shipping code.

## Modules

A module has a name and a `handle(&In) -> Result<Out, ServeError>`.
Pre and post hooks are global: pre runs before the ruleset and may
short-circuit with a status of 400 or higher; post sees the flags and
may rewrite the response. Both are optional and set in config.

Typical module:

```rust
struct Health;
impl Module for Health {
    fn name(&self) -> &'static str { "health" }
    fn handle(&self, req: &In<'_>) -> Result<Out, ServeError> {
        Ok(Out::json(Status::OK, json_out::to_bytes(&json!({"ok": true}))))
    }
}
```

## Atoms and molecules

Atoms are the only mutation API. A pure atom that writes the world is a
defect. Built-in atoms: `signal.get`, `json.pretty`, `resource.get`,
`rules.dry_test` (pure); `json.crud`, `settings.backup`, `server.start`
/ `stop` / `restart`, `rules.reload`, `tunnel.apply` (effectful).

Molecules are named lists of atoms:

```
server.restart = ["server.stop", "server.start"]
ops.dashboard  = ["signal.get", "resource.get"]
```

`allow_write: false` on the atom context turns effectful atoms into
no-op actuators.

## Operator control

The control socket is a Unix stream of JSON lines, mode 0600. Commands:
`status`, `refresh-endpoints`, `rules.reload`, `stop`, `start`,
`restart`, `dry-test-rules`. Default path `/tmp/atomos.sock`.

The operator binary is `atomos-ctl`: an interactive prompt or JSON
lines on stdin.

## HTTP/2 and HTTP/3

`atomos-proto` runs the shared kernel on tokio. It peeks the TCP
stream and dispatches to TLS, HTTP/2 prior knowledge, or HTTP/1.1; UDP
traffic with a QUIC initial is dispatched to HTTP/3.

Measured properties (reference machine, 1000 requests):

- H2 sequential: about 8000 req/s, p50 121 µs.
- H2 multiplexed (64 streams on one connection): about 19,500 req/s.
- H2 wire bytes per request: 149 B first, 12 B steady (HPACK static
  and dynamic table hits).
- H3 sequential: about 5000 req/s, p50 185 µs.
- No application-level head-of-line blocking: a small GET keeps p50
  latency while a 256 KiB upload runs on a sibling stream.

Flow-control windows are the H2/H3 throughput precondition. The
recommended h2 builder settings and the quinn transport config are in
the H2/H3 section of the FDS wiki.

## Planes and plugins

Four planes: the kernel (network, cache, governor), the consumer
modules, the operator control, and the plugin surface. Plugins declare
a manifest; the plugin runtime is Wasm (WIT at `wit/atomos-module.wit`).
Shared objects are refused.

## Configuration

`config.json` holds the runtime configuration: bind address, engine
(`epoll` or `tokio`), static root, rules path, error page, control
socket, memory cap, cache entries and bytes, SO_REUSEPORT, TCP_NODELAY,
pre and post module names. Host facts come from `.atomos/host.json`
(workers, L3-sized cache, refused ports), written by
`scripts/atomos-host.sh`.

Hard bounds in the example config: RSS cap 64 MiB, JSON depth 32, body
262144 bytes, response cache 4096 entries or 16 MiB, 256 rules max.

## Performance and bounds

Release profile: opt-level 3, thin LTO, codegen-units 1, panic abort,
strip. Linker: lld, RELRO, now, noexecstack. CPU flags come from
`scripts/atomos-host.sh` (`target-cpu=native` plus the features in
`/proc/cpuinfo`).

The governor reads RSS through `/proc/self/status` with a 100 ms cache;
`memory_mode: hard` returns 503 over the cap, `degrade` sets
`FLAG_DEGRADED`. Shared atomics are `#[repr(C, align(64))]`. Integers
are formatted into stack buffers on the write path; JSON output uses a
thread-local buffer. Hot-path allocation is zero in the H1 engine,
enforced by the FDS counting-allocator test.

Measured bounds are in [BENCHMARKS.md](BENCHMARKS.md).

## Implementation examples

### 1. Simple API login server

`examples/login_server.rs` is a complete login API: POST
`/api/login` with `{"user","pass"}` returns a bearer token; GET
`/api/session` with the token returns the user; static files serve the
rest.

```sh
cargo run --release --example login_server -- 127.0.0.1:8090
curl -X POST -d '{"user":"alice","pass":"wonderland"}' http://127.0.0.1:8090/api/login
curl -H 'Authorization: Bearer <token>' http://127.0.0.1:8090/api/session
```

The token is a hash of credentials and a request counter; the example
demonstrates the module API, not a security design.

### 2. First web app

`examples/first_app.rs` shows three JSON APIs, a disjoint ruleset,
pre/post hooks, global in-memory state, boot-time load, and the
response cache. Config, rules, and static files are under
`examples/first_app/`.

### 3. Static site

```sh
cargo run --release --example static_site -- 127.0.0.1:8090
```

### 4. Echo API

```sh
cargo run --release --example echo_api -- 127.0.0.1:8090
```

### 5. A module with pre and post hooks

Register `pre` and `post` modules, set `pre_module` and `post_module`
in config, and call `bind_hooks` after insertion. Pre runs before the
ruleset; post runs after the module and sees the flags.

### 6. Response caching

Set `out.cache` on the module output. Global caches serve the encoded
wire form with one writev; named caches are invalidated as a group by
`rules.reload` or a module that mutates the underlying data.

### 7. Operator control

```sh
cargo run --release --bin atomos-ctl -- --config examples/first_app/config.json
echo '{"cmd":"status"}' | cargo run --release --bin atomos-ctl -- --config examples/first_app/config.json --json
```

### 8. HTTP/2 and HTTP/3

```sh
cargo run --release --bin atomos-proto -- --bind 127.0.0.1:8090
curl --http2-prior-knowledge http://127.0.0.1:8090/
curl -k --http3-only https://127.0.0.1:8090/
```

### 9. Plugins

Declare a manifest (see `plugins/example/`), build the Wasm module
against `wit/atomos-module.wit`, and register it in config. Shared
objects are refused.

### 10. Load generation

```sh
cargo run --release --example loadgen -- 127.0.0.1:8090
```

## Limitations

- HTTP/1.1 pipelining is not multiplexed; use HTTP/2 for multiplexing.
- WebSocket upgrade is not implemented.
- The H1 engine is single-request-per-connection-slot, preallocated.
- TLS is served by `atomos-proto` (tokio path), not the H1 epoll path.
- Streaming responses run on the tokio path; the H1 epoll path encodes
  streaming bodies as empty.
