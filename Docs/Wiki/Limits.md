# Limits

- HTTP/1.1 pipelining is sequential. The second request waits until the first response is fully written. Use HTTP/2 for multiplexing.
- WebSocket: the kernel refuses `Upgrade` with 426 and closes the socket. There is no upgrade path.
- The H1 engine is one request per connection slot. The table is preallocated.
- Epoll H1 terminates TLS 1.3 with ALPN http/1.1. HTTP/2 and HTTP/3 remain on atomos-proto.
- H1 `OutBody::Stream` is chunked transfer of bytes already queued when `handle` returns. A live producer that blocks the worker is refused by construction: the worker `try_recv`s and then writes `0\r\n\r\n`.
- H1 `OutBody::File` is `sendfile`. Range requests are served as 206. Invalid ranges are 416.
- Framing: `Content-Length` plus `Transfer-Encoding` is 400. Duplicate `Content-Length` is 400. `Transfer-Encoding` that is not exactly `chunked` is 400. obs-fold is 400. Chunk extensions are 400. Trailers are 400. Absolute-form `Host` mismatch is 400. HTTP/1.1 with no `Host` is 400. HTTP/1.0 with no `Host` is 200.
- Absolute-form is normalized to origin-form before the ruleset. `GET http://127.0.0.1/ HTTP/1.1` plus matching `Host: 127.0.0.1` is 200 and serves the static index.

- Sync `Module::handle` cannot be cancelled. `module_timeout_ms` is a deadline started on the worker; 504 is best-effort after return. Over-budget modules are a contract violation. Wasm fuel, epoch, and the 16 MiB memory limiter map to 504.
- Wasm instances have no WASI filesystem or sockets on the linker. That is the capability story today.

See `Code/tests/smuggling.rs`, `Code/tests/timeouts.rs`, `Code/tests/h1_tls.rs`, `Code/tests/wasm_caps.rs`, and `Code/tests/ws_policy.rs`.
