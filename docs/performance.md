# Performance and bounds

Release: `opt-level=3`, thin LTO, `codegen-units=1`, `panic=abort`, strip.
Linker: lld, RELRO, now, noexecstack. CPU flags come from
`scripts/cpu-rustflags.sh` / `scripts/atomos-host.sh` (`target-cpu=native`
plus features advertised in `/proc/cpuinfo`).

## Hard bounds (config)

| Resource | Default |
|---|---|
| RSS cap | 64 MiB in the example; kernel default 6 GiB |
| JSON depth | 32 |
| Body | 262144 bytes |
| Response cache | 4096 entries / 16 MiB in the example |
| Rules | 256 max |

Governor: `memory_mode` `hard` → HTTP 503 over cap; `degrade` → `FLAG_DEGRADED`.

Shared atomics are `#[repr(C, align(64))]` (`src/align.rs`).

Hot path: `itoa` / `dtoa` into stack buffers. JSON **output** uses a thread-local
`Vec` (`json_out::to_bytes`). Integers are not `format!`-ed on the write path.

## Measured on Broadwell i5-5200U (not shipped)

| Experiment | Result |
|---|---|
| simd-json vs serde_json (typical 56 B) | serde faster |
| tokio-uring vs tokio | tokio p99 better; uring `!Sync` |
| bumpalo serialize (not shipped) | slower than thread-local Vec |
| AVX2 lowercase-then-tokenize | slower than scalar |
| QUIC / HTTP/3 | shipped (UDP same port; ~5k rps on this 2-core — crypto bound) |
| `.so` module reload | skip: JSON ruleset reload is enough |

Empty static site RSS ≈ 3 MiB (bound 64 MiB). first_app with H2+H3 idle
≈ 4.4 MiB, peak HWM 9.3 MiB after mixed load.

Load test of `examples/first_app` (release, loopback, 2026-08-21, **epoll H1**,
4 pinned workers, proto not in-process):

- **HTTP/1.1** `GET /api/health`: 1×p1 **36k** (~28 µs mean); 4×p1 **71k**;
  8×p8 **239k**; 16×p16 **282k**. Notes 8×p8 **232k**. err=0. RSS **~3.2 MiB**.
- Same day, tokio+h2+h3 sharing those cores was 16×p16 **143k** — proto coproduct
  is the rps delta.
- **h2c** / **TLS h2** / **HTTP/3** remain on `atomos-proto` (~36k / ~39k / ~5.3k).
- Python `http.client` was client-bound at ~2–3k rps — do not use it for rps.

Full tables: [bench-first-app.md](bench-first-app.md).

**FDS-backed epoll H1** (wrk, 2026-08-24, same laptop — the engine now
runs on fds-core's reactor/conn-table; release, 4 pinned workers,
HTTP/1.1 keep-alive):

| Workload | Result |
|---|---|
| 18 B static page (wire-cache hit), 4×100 conns | **86.5k req/s**, 1.22 ms avg |
| 64 KB static file (wire-cache hit), 4×100 conns | **23.5k req/s = 1.44 GB/s** |
| 18 B page, 4×500 conns (stress) | **81.3k req/s**, 4.46 ms avg, 0 socket errors; server healthy after |
| 404 (uncached full rules→dispatch→error-page), 4×100 | 24.1k req/s, 4.29 ms avg |

The old 2026-08-21 first_app rows are a module-dispatch workload on the
hand-rolled epoll; the wrk rows are the wire-cache static path on FDS —
different costs, same machine. FDS transport ceilings for context:
`--bench-large` 60 KB one-way 36.2/33.2 Gbps; iperf3 loopback TCP
20–29.7 Gbps; see FDS `docs/engine.md` "Cross-tool benchmarks".

Hot path: pinned current-thread workers, one `write_all` per H1 response,
parse borrows the receive buffer, `SO_REUSEPORT` accept, H1 encoded-byte
cache, H2/H3 semantic `Out` cache, sync `Module` when nothing awaits.

Benches use `[profile.bench]` **without LTO** so `cargo bench` stays fast.

## What to do in a consumer

1. mmap large data; do not walk it at boot.
2. Warm a bounded prefix, then one synthetic request.
3. Keep-alive + `CacheDirective::Global` on static.
4. Upstream work on a bounded channel behind token buckets.

## Ceilings that are not Atomos

On this class of host, further rps is **kernel TCP + mitigations + core count**,
then **TLS/QUIC crypto**, then **the reverse proxy**. io_uring, simd-json, and
software prefetch were measured and rejected. Do not add a userspace NIC stack
inside this crate. Generate host facts with `scripts/atomos-host.sh write`.
