# HTTP/2 and HTTP/3

`atomos-proto` runs the shared kernel on tokio. It peeks the TCP stream and dispatches to TLS, HTTP/2 prior knowledge, or HTTP/1.1. UDP traffic with a QUIC initial goes to HTTP/3.

TLS is on the proto path. The H1 epoll engine does not terminate TLS.

Measured tables: [../Benchmarks.md](../Benchmarks.md).

```
cd Code
cargo run --release --bin atomos-proto -- --bind 127.0.0.1:8090
curl --http2-prior-knowledge http://127.0.0.1:8090/
curl -k --http3-only https://127.0.0.1:8090/
```
