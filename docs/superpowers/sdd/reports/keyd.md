# Report: atomos-keyd

**status:** DONE

## Files changed
- `src/bin/keyd.rs` (replaced stub)
- `src/ops/keyproto.rs` (new)
- `src/ops/mod.rs` (`pub mod keyproto`)

## Implementation
- Protocol: `encode_req` / `decode_req` (`u32be n | u8 kind | [n-1]`), `encode_rep` / `decode_rep` (`u32be n | payload`). No serde. `KIND_SIGN = 1`. `MAX_FRAME = 65536` fail-closed.
- `atomos-keyd --key PEM [--sock PATH]`; default sock `$XDG_RUNTIME_DIR/atomos-keyd.sock` else `/tmp/atomos-keyd.sock`.
- UnixListener 0600, single-threaded accept loop, `jail::peer_euid_ok` (same-EUID / root). Fail closed: drop conn on bad peer, bad kind, truncated/oversize frame, sign error. Missing `--key` → exit 1 `config:`-style error.
- Sign: rustls 0.23 `ring::sign::any_supported_type` + `SigningKey::choose_scheme` + `Signer::sign`. Smoke: P-256 PEM → 71-byte ECDSA signature.

## Tests run
```bash
unset RUSTFLAGS
export CARGO_TARGET_DIR=$HOME/.cache/atomos-target-keyd
cargo test --lib ops::keyproto::
cargo build --bin atomos-keyd
```

**`cargo test --lib ops::keyproto::`:** PASS: 3 passed (`sign_request_roundtrip_bytes`, reply roundtrip, truncated/empty fail-closed).

**`cargo build --bin atomos-keyd`:** FAIL: `src/net/tls.rs:63` unused `pub fn load` under `#![deny(warnings)]` (tls agent left `load` test-only; this brief forbids editing `tls.rs`). Bin **does** compile with command-line `-A dead_code`; runtime smoke (0600 sock, rustls sign, unknown kind → no reply, missing `--key` exit 1) passed.

## Concerns
- **Peer refuse stub:** other-uid not unit-tested (needs a second EUID). Path is `if !jail::peer_euid_ok(fd) { continue; }` (same as `control_std`); documented in `keyd.rs`.
- Protocol has no signature-scheme byte; keyd picks the first rustls scheme the key supports. Payload is the rustls `Signer` **message** (hashed inside ring), not a pre-hashed digest: later `tls.rs` keyclient must send that message.
- Official `cargo build --bin atomos-keyd` stays red until `tls.rs` uses `load` outside `#[cfg(test)]` (e.g. `TlsHold::get`).
