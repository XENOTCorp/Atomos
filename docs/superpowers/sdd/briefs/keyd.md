# Brief: atomos-keyd

Repo: `/home/xenot/Projects/Atomos`
Cargo.toml already has `[[bin]] name = "atomos-keyd" path = "src/bin/keyd.rs"`: you MUST create that file so the crate compiles.

You MAY create:
- `src/bin/keyd.rs`
- `src/ops/keyproto.rs` (encode/decode only, no rustls required)
You MAY edit `src/ops/mod.rs` (`pub mod keyproto`)
Do NOT edit tls.rs, jail.rs, plugin/, route.rs, Cargo.toml, config.rs (`keyd_sock` already on Config).
Do NOT commit. Do NOT bind 8082. Do NOT bind 8090.

CARGO_TARGET_DIR=$HOME/.cache/atomos-target ; unset RUSTFLAGS

## Protocol (fixed, no serde on datapath)
```
req: u32be n | u8 kind | [n-1 bytes payload]
kind 1 = sign digest
rep: u32be n | payload
```
`keyproto::encode_req(kind, payload) -> Vec<u8>`
`keyproto::decode_req(&[u8]) -> Option<(u8, &[u8])>`
Same for replies.

## keyd.bin
- argv: `--key PEM --sock PATH` (default sock `$XDG_RUNTIME_DIR/atomos-keyd.sock` else `/tmp/atomos-keyd.sock`)
- UnixListener 0600, SO_PEERCRED same EUID (copy jail::peer_euid_ok pattern)
- Load rustls PEM private key, sign with rustls 0.23 ring SigningKey if straightforward; if rustls SigningKey plumbing is too thick, implement a working RSA/ECDSA sign using `ring` already pulled by rustls, or return a clear Config error in tests
- Single-threaded
- Fail closed

## Tests
`sign_request_roundtrip_bytes` in keyproto.
Stub `peer` refuse: document.

Default `cargo test --lib` MUST compile: `src/bin/keyd.rs` must exist even if key crypto is a stub that signs SHA-256 HMAC-like with file bytes for tests: prefer real rustls sign.

Report: `/home/xenot/Projects/Atomos/docs/superpowers/sdd/reports/keyd.md`
No subagents. No git commit.
`cargo test --lib ops::keyproto::`
`cargo build --bin atomos-keyd`
