# Brief: Landlock restrict_self + seccomp BPF

Repo: `/home/xenot/Projects/Atomos`
ONLY file you may edit: `src/ops/jail.rs`
Do NOT commit. Do NOT bind 8082. Do NOT edit Cargo.toml.

CARGO_TARGET_DIR=$HOME/.cache/atomos-target
unset RUSTFLAGS

## Landlock
Replace `landlock_restrict` so it actually calls:
- `landlock_create_ruleset` (syscall 444 on x86_64)
- `landlock_add_rule` (445) for `cfg.static_root`, `cfg.rules_path` parent, `cfg.control_socket` parent
- `prctl(PR_SET_NO_NEW_PRIVS)` already done
- `landlock_restrict_self` (446)

Handled access: READ_FILE | READ_DIR | WRITE_FILE (notes/rules) as needed for a static origin.
If errno is ENOSYS or EOPNOTSUPP: `tracing::warn` and return Ok (old kernel).
If `cfg.landlock` is true and errno is anything else: return `ServeError::Config`.

Unit test (linux): `landlock_restrict` on a tempdir does not panic; ENOSYS path ok.
Do not enable landlock in existing tests' JSON (default is false).

## Seccomp
Replace `seccomp_allowlist` so it installs a BPF filter via `prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &sock_fprog)` OR `syscall(SYS_seccomp, SECCOMP_SET_MODE_FILTER, ...)`.
Allow `SECCOMP_ALLOW` plus whatever extra syscalls a Rust/epoll worker actually needs so `cargo test --lib jail::` still passes when `seccomp` is false (default).
If you cannot prove a filter that does not SIGSYS `cargo test`, keep default off and add a test that:
- `SECCOMP_ALLOW` contains epoll_wait/pwait
- does NOT contain SYS_execve
- a helper `seccomp_filter_bytes()` returns a non-empty prog

Do not turn `cfg.seccomp` default on.

Write report to `/home/xenot/Projects/Atomos/docs/superpowers/sdd/reports/jail.md` with status DONE/BLOCKED, tests run, concerns.
No subagents. No git commit.
