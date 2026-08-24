# Report: TLS tickets + OCSP file staple

**Status:** done  
**Date:** 2026-08-20

## Changes

Only `src/net/tls.rs` (no `tls_tickets.rs`).

- Added `load_with_opts(cert, key, ocsp: Option<&[u8]>, ticket_lifetime_secs: u64)`.
- `load(cert, key)` now calls `load_with_opts(..., None, 86400)`.
- OCSP: when `ocsp` is `Some`, uses rustls 0.23 `with_single_cert_with_ocsp`.
- Tickets: installs rotating ticketer via `rustls::crypto::ring::Ticketer::new()` (`ProducesTickets` / `ServerConfig::ticketer`).
- Non-zero `ticket_lifetime_secs` wraps the ring ticketer in `LifetimeTickets` so `ProducesTickets::lifetime()` reports the configured value. `0` leaves rustls/ring default.
- `TlsHold` stores `ticket_lifetime_secs` (default `86400`) and `get()` builds via `load_with_opts`.

Config fields `tls_ocsp` / `tls_ticket_lifetime_secs` left untouched (already in `config.rs`); wiring into `serve` / proto is out of this brief.

## Tests

```bash
unset RUSTFLAGS
export CARGO_TARGET_DIR=$HOME/.cache/atomos-target-tls
cargo test --lib net::tls::
```

Result: **3 passed** (`self_signed_builds_tcp_and_quic`, `load_with_opts_none_ocsp_matches_load`, `load_with_opts_accepts_ocsp_bytes`).

## Concerns

- Ring’s `Ticketer::new()` hardcodes a 6h key-rotation interval (advertised default 12h). `LifetimeTickets` only overrides the advertised lifetime; encrypt/decrypt still use the ring rotator. Matching rotation period to `ticket_lifetime_secs` needs a custom `TicketRotator` generator and cannot use crate `ring` directly without editing `Cargo.toml` (forbidden here). Lifetime remains stored on `TlsHold` for proto to refine later.
- `tls_ocsp` path and `tls_ticket_lifetime_secs` from `Config` are not wired into `TlsHold::new` / serve (brief forbids editing those files).
