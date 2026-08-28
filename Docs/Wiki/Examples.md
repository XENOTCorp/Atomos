# Examples

Run every example from `Code/`.

## login_server

Complete login API. `POST /api/login` with `{"user","pass"}` returns a bearer token. `GET /api/session` with the token returns the user. Static files serve the rest.

```
cargo run --release --example login_server -- 127.0.0.1:8090
```

The token is a hash of credentials and a request counter. The example shows the module API. It is not a security design.

## first_app

Three JSON APIs, a disjoint ruleset, pre/post hooks, global in-memory state, boot-time load, and the response cache. Files live under `Code/examples/first_app/`.

```
cargo run --release --example first_app -- 127.0.0.1:8090
```

## static_site

```
cargo run --release --example static_site -- 127.0.0.1:8090
```

## echo_api

```
cargo run --release --example echo_api -- 127.0.0.1:8090
```

## loadgen

```
cargo run --release --example loadgen -- 127.0.0.1:8090
```

## Caching

Set `out.cache` on the module output. Global caches serve the encoded wire form. Named caches are invalidated as a group by `rules.reload`.

## Control

See [Control.md](Control.md).
