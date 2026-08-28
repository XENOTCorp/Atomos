# Limits

- HTTP/1.1 pipelining is not multiplexed. Use HTTP/2 for multiplexing.
- WebSocket upgrade is not implemented.
- The H1 engine is one request per connection slot. The table is preallocated.
- TLS is served by `atomos-proto` (tokio path). The H1 epoll path does not terminate TLS.
- Streaming responses run on the tokio path. The H1 epoll path encodes streaming bodies as empty.
