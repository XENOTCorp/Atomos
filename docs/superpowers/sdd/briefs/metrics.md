# Brief: metrics + access log

Repo: `/home/xenot/Projects/Atomos`
You MAY create:
- `src/kernel/metrics.rs`
- `src/net/access_log.rs`
You MAY edit:
- `src/kernel/mod.rs` (pub mod metrics)
- `src/kernel/route.rs` (increment atomics; cache hit still skips module)
- `src/lib.rs` (static_router wires Metrics)
- `src/net/mod.rs` (pub mod access_log)
- `src/net/epoll.rs` (after successful encode/write, if cfg.access_log, log one line)
Do NOT edit config.rs (fields `access_log` already exist).
Do NOT commit. Do NOT bind 8082. Do NOT touch jail.rs, tls.rs, plugin/, bins.

CARGO_TARGET_DIR=$HOME/.cache/atomos-target ; unset RUSTFLAGS

## Metrics
`Metrics` with `LineAtomicU64` from `crate::align`: `requests`, `hits`, `misses`, `bytes_out`.
`snapshot()` copies loads (Relaxed).
On `Router::dispatch` and `dispatch_async`:
- every call: requests += 1
- cache hit: hits += 1 then return (still no module)
- miss that runs a module: misses += 1
- bytes_out += out.body.len() after produce (including hits if Out available; on wire cache hit use hit body len if present, else skip bytes)

Add Module `MetricsMod` in metrics.rs: `handle` returns Prometheus text:
```
atomos_requests N
atomos_cache_hits N
atomos_cache_misses N
atomos_bytes_out N
```
using itoa, no format! on numbers if easy.

Test: `snapshot_is_pure` (two snapshots equal after one increment).
Test: dispatch twice with Global cache → misses==1, hits>=1, module counter 1.

## Access log
`access_log::line(method, path, status, body_len, dst: &mut Vec<u8>)` writes one CLF-ish or logfmt line, no `format!` (itoa).
epoll: after `encode_response` / cache wire copy, if `router.cfg.access_log`, write to stderr or a helper that currently writes to a test Vec. Do not allocate unbounded. Skip if FLAG_METRICS_SKIP? No: log after encode always when cfg.access_log.

Test `line_contains_path_and_status`.

Report: `/home/xenot/Projects/Atomos/docs/superpowers/sdd/reports/metrics.md`
No subagents. No git commit.
