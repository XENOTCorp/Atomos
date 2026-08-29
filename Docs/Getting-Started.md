# Getting Started

Linux. Rust 1.97.1 or later. A C compiler.

## 1. Check the compiler

```
uname -s
rustc --version
```

`uname -s` must print `Linux`. `rustc` must be 1.97.1 or later.

## 2. Compile on this machine

From the repository root:

```
./compile.sh
```

The script writes two local files and builds release binaries:

- `Code/.cargo/config.toml` — CPU and linker flags for this host
- `Code/.atomos/host.json` — worker count and L3 cache size for this host

Git ignores both files. Do not copy them between machines.

Other commands: `./compile.sh write`, `./compile.sh test`, `./compile.sh print`.

Full compile notes: [Wiki/Compile.md](Wiki/Compile.md).

## 3. Run tests

```
cd Code
cargo test
```

Cargo.toml lives in `Code/`. Run Cargo from `Code/`, not from the repository root.
`./compile.sh` already changes into `Code/` for you.

## 4. Run the login server example

```
cd Code
cargo run --release --example login_server -- 127.0.0.1:8090
```

The example is a complete API. `POST /api/login` returns a bearer token. `GET /api/session` validates the token. Static files serve the rest.

## 5. Send three requests

Open a second terminal:

```
curl -X POST -d '{"user":"alice","pass":"wonderland"}' http://127.0.0.1:8090/api/login
curl -H 'Authorization: Bearer <token>' http://127.0.0.1:8090/api/session
curl http://127.0.0.1:8090/
```

Replace `<token>` with the token from the first response.

## 6. Read the wiki

Open [Wiki/Home.md](Wiki/Home.md).

Architecture, pre/post hooks, and hot-swap: [Wiki/Architecture.md](Wiki/Architecture.md).

## 7. HTTP/2 and HTTP/3

`atomos-proto` serves HTTP/2 and HTTP/3 on tokio.

```
cd Code
cargo run --release --bin atomos-proto -- --bind 127.0.0.1:8090
curl --http2-prior-knowledge http://127.0.0.1:8090/
curl -k --http3-only https://127.0.0.1:8090/
```

More examples: `first_app`, `static_site`, `echo_api`, `loadgen`. See [Wiki/Examples.md](Wiki/Examples.md).
