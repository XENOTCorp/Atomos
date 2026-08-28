# Getting Started

Work in `Code/`. FDS crates are in `Code/FDS`.

## 1. Install Rust

Rust 1.97.1 or later on Linux.

```
rustc --version
```

## 2. Open a shell in Code/

```
cd Code
```

Cargo.toml lives here. Run Cargo from `Code/`, not the repository root.

## 3. Run tests

```
cargo test
```

## 4. Run the login server example

```
cargo run --release --example login_server -- 127.0.0.1:8090
```

The example is a complete API. `POST /api/login` returns a bearer token. `GET /api/session` validates the token. Static files serve the rest.

## 5. Send three requests

In another terminal:

```
curl -X POST -d '{"user":"alice","pass":"wonderland"}' http://127.0.0.1:8090/api/login
curl -H 'Authorization: Bearer <token>' http://127.0.0.1:8090/api/session
curl http://127.0.0.1:8090/
```

Replace `<token>` with the token from the first response.

## 6. Read the wiki

Open [Wiki/Home.md](Wiki/Home.md).

## 7. HTTP/2 and HTTP/3

`atomos-proto` serves HTTP/2 and HTTP/3 on tokio.

```
cargo run --release --bin atomos-proto -- --bind 127.0.0.1:8090
curl --http2-prior-knowledge http://127.0.0.1:8090/
curl -k --http3-only https://127.0.0.1:8090/
```

More examples: `first_app`, `static_site`, `echo_api`, `loadgen`. See [Wiki/Examples.md](Wiki/Examples.md).
