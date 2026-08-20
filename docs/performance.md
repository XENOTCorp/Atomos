# Performance and bounds

Release: `opt-level=3`, thin LTO, `codegen-units=1`, `panic=abort`, strip.
Linker: lld, RELRO, now, noexecstack. `target-cpu=native` (Broadwell on XENOT
compute: AVX2 only, no AVX-512).

## Hard bounds (config)

| Resource | Default |
|---|---|
| RSS cap | 64 MiB in the example; kernel default 6 GiB |
| JSON depth | 32 |
| Body | 262144 bytes |
| Response cache | 4096 entries / 16 MiB in the example |
| Rules | 256 max |
| Request bumpalo | 4096 bytes |

Governor: `memory_mode` `hard` → HTTP 503 over cap; `degrade` → `FLAG_DEGRADED`.

Shared atomics are `#[repr(C, align(64))]` (`src/align.rs`).

Hot path: `itoa` / `dtoa` into stack buffers. JSON **output** uses a thread-local
`Vec` (`json_out::to_bytes`). Integers are not `format!`-ed on the write path.

## Measured on Broadwell i5-5200U (not shipped)

| Experiment | Result |
|---|---|
| simd-json vs serde_json (typical 56 B) | serde faster |
| tokio-uring vs tokio | tokio p99 better; uring `!Sync` |
| bumpalo serialize | slower than thread-local Vec |
| AVX2 lowercase-then-tokenize | slower than scalar |
| QUIC | skip: TLS already at cloudflared |
| `.so` module reload | skip: JSON ruleset reload is enough |

Empty static site RSS ≈ 3 MiB (bound 64 MiB).

Benches use `[profile.bench]` **without LTO** so `cargo bench` stays fast.

## What to do in a consumer

1. mmap large data; do not walk it at boot.
2. Warm a bounded prefix, then one synthetic request.
3. Keep-alive + `CacheDirective::Global` on static.
4. Upstream work on a bounded channel behind token buckets.
