# Maintain

Procedures for a maintainer of this tree. Work in `Code/` unless a step says otherwise.

## Copy FDS crates again

Atomos vendors two FDS library crates as ordinary files. The crates are `fds` and `mol`. They are not git submodules.

1. Have a sibling FDS tree at `../FDS`.
2. From the Atomos repository root, copy the two crates and the templates used by Mol's compile-verification tests:

```
rm -rf Code/FDS/crates/fds Code/FDS/crates/mol
cp -a ../FDS/Code/crates/fds Code/FDS/crates/fds
cp -a ../FDS/Code/crates/mol Code/FDS/crates/mol
cp -a ../FDS/Code/templates/. Code/FDS/templates/
rm -rf Code/FDS/crates/fds/target Code/FDS/crates/mol/target
```

3. Rewrite workspace keys in both `Cargo.toml` files. FDS uses `version.workspace = true`. After the copy, the crates belong to the Atomos workspace, not the FDS workspace. Set:

```
version = "0.1.0"
edition = "2021"
rust-version = "1.97.1"
license = "Apache-2.0"
authors = ["XENOT Corporation"]
```

4. Keep `mol = { path = "../mol" }` in `Code/FDS/crates/fds/Cargo.toml`.
5. Keep `fds = { path = "FDS/crates/fds", default-features = false }` in `Code/Cargo.toml`.
6. Preserve local FDS fixes when updating: legacy io_uring must drain partial TCP writes before reading again and retain in-flight buffers until ring teardown. Pools must initialize every slot before allocation; raw slot access/release and generic zero-initialization must remain unsafe. SPSC access requires exclusive operations or unique split endpoints. Keep transport changes focused and covered by regression tests.
7. Strip U+2014 and U+2013 from the copied comments if the FDS source still uses them.
8. Run `cd Code && cargo fmt --all && cargo test --locked -p mol && cargo test --locked -p fds --no-default-features`.

All three crates share `Code/Cargo.lock` and `Code/target/`. Do not generate separate vendored lockfiles. To test all FDS transports, install `libsctp-dev` (Debian/Ubuntu) and run `cargo test --locked -p fds --all-features`.

## Generate device files

From the repository root:

```
./compile.sh write
```

`compile.sh` reads this machine. It writes `Code/.cargo/config.toml` and `Code/.atomos/host.json`. Git ignores both files. Do not copy them between hosts.

Direct scripts, same result:

```
cd Code
scripts/cpu-rustflags.sh write .
scripts/atomos-host.sh write .
```

## Tests and clippy

From `Code/`:

```
cargo fmt --all --check
cargo test --locked
cargo test --locked --release
cargo test --locked --all-features
cargo test --locked -p mol
cargo test --locked -p mol -- --ignored --test-threads=1
cargo test --locked -p fds --no-default-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
scripts/test-host.sh
```

CI uses the Rust version pinned in `rust-toolchain.toml`. When changing it, update `RUST_TOOLCHAIN` in `.github/workflows/ci.yml` as well. Package `rust-version` declares the minimum supported compiler and should only change when necessary.

Strict HTTP/2 conformance (download h2spec 2.6.0 as in the workflow):

```
scripts/test-h2spec.sh /absolute/path/to/h2spec
```

The script checks server readiness, cleans up its server process, and prints server logs on failure. `H2SPEC_PORT` overrides the default port 8090.

## Request-path review

- `src/kernel/cache/`: router-scoped storage, invalidation epochs, conditional responses, and bounded accounting. Cache keys do not include headers: private, range, body-bearing, header-constrained, and hooked requests must not reuse public responses.
- `src/kernel/route/`: shared admission/hook policy with separate sync, async, and streaming handler invocation. Preserve one admission and one guard release per dispatched request.
- `src/net/epoll/`: listener/slot lifecycle, request processing, bounded buffers, and output queues. Preserve generation-token checks and response order under backpressure.
- `src/net/file_body.rs`: bounded positional reads for Tokio transports; do not reintroduce whole-file buffering.
- `src/kernel/static_files/open.rs`: root-relative descriptor opens; never canonicalize a path and reopen it by name.

`tests/sandbox.rs` launches the real H1 binary with seccomp, Landlock, and both together on Linux x86_64. It verifies small and sendfile response contents and catches worker-startup syscall omissions. It is a functionality regression, not a complete sandbox security audit; Landlock may fall back on unsupported kernels.

## Performance changes

Keep the zero-allocation tests enabled. File-transfer changes must preserve byte contents, ranges, and pipelined ordering under backpressure, not merely response lengths. Run `tests/http_semantics.rs` and `tests/h1_tls.rs` when changing epoll output handling.

Throughput claims require repeated measurements on the same dedicated host with the same compiler, configuration, payloads, and load. Do not use noisy GitHub-hosted runner timings as a performance ranking; see [Benchmarks.md](Benchmarks.md).

## Layout

Root metadata: `README.md`, `LICENSE`, `.gitignore`, `compile.sh`. Content directories: `Code/` and `Docs/`.
