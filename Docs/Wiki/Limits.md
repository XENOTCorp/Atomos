# Limits

- HTTP/1.1 pipelining is sequential. The second request waits until the first response is fully written. Use HTTP/2 for multiplexing.
- WebSocket: the kernel refuses `Upgrade` with 426 and closes the socket. There is no upgrade path.
- The H1 engine is one request per connection slot. The table is preallocated.
- TLS on the public H1 epoll engine is not in this tree yet. `atomos-proto` terminates TLS for HTTP/1.1, HTTP/2, and HTTP/3. H1 TLS on pinned workers is the next engine change. Until it lands, proto is the TLS listener.
- H1 `OutBody::Stream` is chunked transfer of bytes already queued when `handle` returns. A live producer that blocks the worker is refused by construction: the worker `try_recv`s and then writes `0\r\n\r\n`.
- H1 `OutBody::File` is `sendfile`. Range requests are served as 206. Invalid ranges are 416.
- Framing: `Content-Length` plus `Transfer-Encoding` is 400. Duplicate `Content-Length` is 400. `Transfer-Encoding` that is not exactly `chunked` is 400. obs-fold is 400. Chunk extensions are 400. Trailers are 400. Absolute-form `Host` mismatch is 400.

See `Code/tests/smuggling.rs` and `Code/tests/ws_policy.rs`.
