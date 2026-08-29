# HTTP/2 and HTTP/3

`atomos-proto` runs the shared kernel on tokio. It peeks the TCP stream and dispatches to TLS, HTTP/2 prior knowledge, or HTTP/1.1. UDP traffic with a QUIC initial goes to HTTP/3.

TLS on `atomos-proto` covers HTTP/1.1, HTTP/2, and HTTP/3. The H1 epoll engine terminates TLS 1.3 with ALPN `http/1.1` when `h1_tls` is set.

Measured tables: [../Benchmarks.md](../Benchmarks.md).

```
cd Code
cargo run --release --bin atomos-proto -- --bind 127.0.0.1:8090
curl --http2-prior-knowledge http://127.0.0.1:8090/
curl -k --http3-only https://127.0.0.1:8090/
```
