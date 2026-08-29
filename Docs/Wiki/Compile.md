# Compile

Atomos compiles on Linux. Device facts stay out of the source tree.

`compile.sh` reads this machine and writes two ignored files:

| File | Role |
|---|---|
| `Code/.cargo/config.toml` | `target-cpu=native`, SIMD from `/proc/cpuinfo`, linker |
| `Code/.atomos/host.json` | worker count, CPU pin, L3-sized cache |

Do not copy those files to a different machine. Run `compile.sh` on each host.

## Procedure

1. Install Linux.
2. Install Rust 1.97.1 or a later version.
3. Install a C compiler (`cc`, `gcc`, or `clang`).
4. Install `lld` or `mold` if you want a fast linker. The script uses the compiler linker when neither is present.
5. Open a shell in the repository root.
6. Run `./compile.sh`.

`./compile.sh` writes the device files and builds the release binaries.

Other commands:

```
./compile.sh write    # write device files only
./compile.sh test     # write device files and run cargo test
./compile.sh print    # print flags and host.json
```

From `Code/`:

```
./scripts/compile.sh
```

## What the script detects

| Fact | Source |
|---|---|
| CPU | `uname -m` and `/proc/cpuinfo` |
| SIMD on x86 | `flags:` line. AVX names only. No microarch string |
| SIMD on aarch64 | `target-cpu=native`. No AVX flags |
| Rust target | `rustc -vV` host triple |
| C compiler | `ATOMOS_CC`, else `cc`, `gcc`, `clang` |
| Linker | `ATOMOS_LD`, else `lld`, `mold`, compiler default |
| Workers | `nproc` |
| Cache bytes | L3 size in sysfs, else 16 MiB |

The kernel default for workers is `available_parallelism`. The host overlay overwrites that value after `compile.sh write`.

## Environment

| Name | Role |
|---|---|
| `ATOMOS_CC` | C compiler name for `linker =` |
| `ATOMOS_LD` | `lld`, `mold`, or empty |
| `ATOMOS_HOST` | Runtime path to a host overlay |
| `ATOMOS_REFUSE_PORTS` | Comma-separated ports the process will not bind |
| `RUSTFLAGS` | Overrides `.cargo/config.toml`. `compile.sh` unsets it |

## Cargo without the script

```
cd Code
cargo test
```

This path uses the compiler default CPU. It is correct on every Linux host. It is slower than a native build.

## Git

Git ignores `Code/.cargo/config.toml` and `**/.atomos/`. Those files are local.
