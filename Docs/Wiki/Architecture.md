# Architecture

Four planes:

1. kernel: network types, rules, cache, governor.
2. net: listen, parse, encode, engines.
3. ops: atoms, Unix control, supervisor.
4. plugin: directory manifests. Wasm slot. Native `.so` is refused.

Request path:

```
optional pre module
parse (httparse / h2 / h3)
first-byte and depth/size scan of JSON bodies
ruleset match
module(In) -> Out
optional post module
optional response cache
encode
```

Pinned workers accept, parse, and write on their own core. Blocking work belongs in the consumer. Do not block the request path.

The H1 engine runs one FDS reactor per pinned worker. It binds FDS TCP listeners with SO_REUSEPORT. Per-connection HTTP state is keyed by FDS connection tokens.

HTTP/1.1 keep-alive uses the encoded byte cache. HTTP/2 and HTTP/3 use the semantic `Out` cache with the same epoch.
