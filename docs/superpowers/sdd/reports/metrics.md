# Report: metrics + access log

## status

done

## files changed

- created: `src/kernel/metrics.rs`
- created: `src/net/access_log.rs`
- edited: `src/kernel/mod.rs`
- edited: `src/kernel/route.rs`
- edited: `src/lib.rs`
- edited: `src/net/mod.rs`
- edited: `src/net/epoll.rs`
- created: `docs/superpowers/sdd/reports/metrics.md`

## tests run + result

```
unset RUSTFLAGS
export CARGO_TARGET_DIR=$HOME/.cache/atomos-target-metrics
cargo test --lib
```

- `kernel::metrics::tests::snapshot_is_pure` — pass
- `kernel::metrics::tests::dispatch_hit_miss_counters` — pass (misses==1, hits>=1, module counter 1)
- `net::access_log::tests::line_contains_path_and_status` — pass
- full `--lib`: **83 passed**, 0 failed

## concerns

- Epoll **wire** cache hits bypass `dispatch`, so they do not increment `Metrics` (`requests`/`hits`/`bytes_out`). Access log on that path uses `cache.get` for status/body len when still present, else status 200 and bytes 0.
- Access log with `cfg.access_log` copies `path` once per dispatch response (needed because encode runs after `In` borrows end).
- Host `.cargo/config.toml` injects `-avx10.1/-avx10.2` rustflags; rustc emits unstable-feature notes (not crate lint failures).
