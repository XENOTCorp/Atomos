# Maintain

Procedures for a maintainer of this tree. Work in `Code/` unless a step says otherwise.

## Copy FDS crates again

Atomos vendors two FDS library crates as ordinary files. The crates are `fds` and `mol`. They are not git submodules.

1. Have a sibling FDS tree at `../FDS`.
2. From the Atomos repository root, copy the two crates:

```
rm -rf Code/FDS/crates/fds Code/FDS/crates/mol
cp -a ../FDS/Code/crates/fds Code/FDS/crates/fds
cp -a ../FDS/Code/crates/mol Code/FDS/crates/mol
rm -rf Code/FDS/crates/fds/target Code/FDS/crates/mol/target
```

3. Rewrite workspace keys in both `Cargo.toml` files. FDS uses `version.workspace = true`. After the copy, the crates are not in the FDS workspace. Set:

```
version = "0.1.0"
edition = "2021"
rust-version = "1.97.1"
license = "Apache-2.0"
```

4. Keep `mol = { path = "../mol" }` in `Code/FDS/crates/fds/Cargo.toml`.
5. Keep `fds = { path = "FDS/crates/fds", default-features = false }` in `Code/Cargo.toml`.
6. Do not change FDS control flow, types, or syscalls.
7. Strip U+2014 and U+2013 from the copied comments if the FDS source still uses them.
8. Run `cd Code && cargo test`.

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
cargo test
cargo test --release
cargo clippy --all-targets -- -D warnings
```

## Layout

Root metadata: `README.md`, `LICENSE`, `.gitignore`, `compile.sh`. Content directories: `Code/` and `Docs/`.
