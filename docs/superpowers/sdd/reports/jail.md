# Report: Landlock restrict_self + seccomp BPF

**status:** DONE

## Files changed
- `src/ops/jail.rs` (only)

## Implementation
- **Landlock:** `landlock_restrict` calls syscalls 444/445/446 (`create_ruleset`, `add_rule`, `restrict_self`). Handled access = `READ_FILE | READ_DIR | WRITE_FILE`. Rules for `static_root` (read), `rules_path` parent and `control_socket` parent (read+write). `ENOSYS`/`EOPNOTSUPP` → `tracing::warn` + `Ok`; other errno → `ServeError::Config`.
- **Seccomp:** `seccomp_allowlist` installs a classic BPF filter via `SYS_seccomp(SECCOMP_SET_MODE_FILTER)` with `prctl(PR_SET_SECCOMP)` fallback. `seccomp_filter_bytes()` returns a non-empty prog. `SECCOMP_ALLOW` keeps epoll_wait/pwait, excludes `execve`, plus worker open/runtime syscalls (commented). Defaults remain off (`cfg.landlock` / `cfg.seccomp`).

## Tests run
```bash
unset RUSTFLAGS
CARGO_TARGET_DIR=$HOME/.cache/atomos-target-jail cargo test --lib jail::
```
**Result:** PASS — 3 passed (seccomp allowlist + filter bytes; landlock_restrict on tempdir via fork; prepare_socket_dir).

## Concerns
- Seccomp install is unproven under `cargo test` with `seccomp: true` (would SIGSYS on missing syscalls); default stays false; filter reviewed via `seccomp_filter_bytes` / allowlist unit tests only.
- Landlock unit test forks a child so `restrict_self` does not sandbox the multi-threaded test process.
- Allowlist may still be incomplete for a full production epoll worker (e.g. `clone3`/`statx`/`rseq`); extend with a named comment before enabling `seccomp` in production JSON.
