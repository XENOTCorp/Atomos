# Brief: TLS tickets + OCSP file staple

Repo: `/home/xenot/Projects/Atomos`
ONLY file: `src/net/tls.rs` (you may add `src/net/tls_tickets.rs` if cleaner)
Do NOT edit config.rs: fields `tls_ocsp: Option<PathBuf>`, `tls_ticket_lifetime_secs: u64` (default 86400) already exist.
Do NOT edit jail, plugin, bins, Cargo.toml.
Do NOT commit. Do NOT bind 8082.

CARGO_TARGET_DIR=$HOME/.cache/atomos-target ; unset RUSTFLAGS

## Tickets
When building `RustlsServer`, set session ticket lifetime from a parameter. Add `load_with_opts(cert, key, ocsp: Option<&[u8]>, ticket_lifetime_secs: u64)`.
Keep `load(cert, key)` calling `load_with_opts(..., None, 86400)` so existing test `self_signed_builds_tcp_and_quic` still passes.

If rustls 0.23 exposes `ticketer` / `ProducesTickets`, install a rotating ticketer. If not, set `max_early_data` already there and document in report that lifetime is stored on TlsHold for proto to use later.

## OCSP
If ocsp bytes Some, attach to certificate via rustls `with_single_cert_with_ocsp` if that API exists in 0.23; else ignore with a test that `load_with_opts` accepts Some(&[0u8; 8]) without panic (may still succeed without stapling).

## Tests
Keep `self_signed_builds_tcp_and_quic`.
Add `load_with_opts_none_ocsp_matches_load`.

Report: `/home/xenot/Projects/Atomos/docs/superpowers/sdd/reports/tls.md`
No subagents. No git commit.
`cargo test --lib net::tls::`
