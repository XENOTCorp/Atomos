# Atomos

An HTTP kernel in Rust. Disjoint JSON rules map `(method, path)` to one
module. Modules are plain functions from `In` to `Out`. The kernel
accepts TCP, parses headers, scans JSON depth, matches the ruleset,
runs the module, and writes the response. There is no regex router, no
middleware stack, and no per-request heap allocation on the HTTP/1.1
path.

Two engines share one kernel:

- `atomos` serves HTTP/1.1 on the FDS epoll transport.
- `atomos-proto` serves HTTP/1.1, HTTP/2, HTTP/3, and TLS on tokio.

Atomos is built on [FDS](https://github.com/ascendnoosphere/FDS), the
transport engine. See [BENCHMARKS.md](BENCHMARKS.md) for measured
comparisons against nginx, h2o, Caddy, Seastar, axum, Hyper, and
nghttpd, and [WIKI.md](WIKI.md) for features, architecture, and
implementation examples.

## Quick start

The login server example is a complete API: `POST /api/login` returns a
bearer token, `GET /api/session` validates it, static files serve the
rest.

```sh
cargo run --release --example login_server -- 127.0.0.1:8090
```

In another terminal:

```sh
curl -X POST -d '{"user":"alice","pass":"wonderland"}' http://127.0.0.1:8090/api/login
curl -H 'Authorization: Bearer <token>' http://127.0.0.1:8090/api/session
curl http://127.0.0.1:8090/
```

HTTP/2 and HTTP/3:

```sh
cargo run --release --bin atomos-proto -- --bind 127.0.0.1:8090
curl --http2-prior-knowledge http://127.0.0.1:8090/
curl -k --http3-only https://127.0.0.1:8090/
```

More examples: `first_app` (three JSON APIs with rules, pre/post hooks,
and caching), `static_site`, `echo_api`, `loadgen`.

## Requirements

- Linux (the H1 engine is epoll)
- Rust 1.97 or later
- FDS as a path dependency (`fds-core`)

## Documentation

- [WIKI.md](WIKI.md): features, architecture, and implementation examples
- [BENCHMARKS.md](BENCHMARKS.md): apples-to-apples measurements
- [examples/](examples/): runnable applications

## License

MIT. Copyright (c) 2026 Alex @AscendNoosphere, XENOT Corporation.
