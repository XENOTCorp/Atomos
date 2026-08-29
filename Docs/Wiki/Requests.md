# Requests

`In` borrows the receive buffer: method, path, query, headers, body, peer, flags.

`Out` owns the response: status, reason, headers, body, cache directive, flags.

Body types:

- `Empty`
- `Raw`
- `Json`
- `Stream`
- `File`

The H1 epoll path sends `File` with `sendfile`. Range requests return 206. Invalid ranges return 416. HEAD keeps `Content-Length` and omits body bytes. The tokio paths materialize the file in memory. HTTP/2, HTTP/3, and TLS need the bytes.

H1 `Stream` is chunked transfer of bytes already queued when `handle` returns.

Cache directives:

- `No` (default): the response is never cached.
- `Global { ttl_ms }`: cached per worker, keyed by the wire form.
- `Named { ruleset, ttl_ms }`: cached per named set. Invalidation is by name.

The response cache is per worker. Entries and bytes are bounded in config. A cache hit is one write of the stored wire bytes.
