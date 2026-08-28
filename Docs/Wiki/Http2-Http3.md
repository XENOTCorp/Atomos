# HTTP/2 and HTTP/3

`atomos-proto` runs the shared kernel on tokio. It peeks the TCP stream. It dispatches to TLS, HTTP/2 prior knowledge, or HTTP/1.1. UDP traffic with a QUIC initial goes to HTTP/3.

TLS is on the proto path. The H1 epoll engine does not terminate TLS.

Measured properties on the reference machine (1000 requests, in-repo `bench_h23`):

- H2 sequential: about 8000 req/s, p50 121 µs.
- H2 multiplexed (64 streams on one connection): about 19,500 req/s.
- H2 wire bytes per request: 149 B first, 12 B steady.
- H3 sequential: about 5000 req/s, p50 185 µs.

A small GET keeps p50 latency while a 256 KiB upload runs on a sibling stream.

Full tables: [../Benchmarks.md](../Benchmarks.md).

```
cd Code
cargo run --release --bin atomos-proto -- --bind 127.0.0.1:8090
curl --http2-prior-knowledge http://127.0.0.1:8090/
curl -k --http3-only https://127.0.0.1:8090/
```
