# Performance

Release profile: opt-level 3, thin LTO, codegen-units 1, panic abort, strip.

CPU flags come from `./compile.sh` (`Code/scripts/cpu-rustflags.sh`). The script sets `target-cpu=native` and SIMD names from `/proc/cpuinfo`. It does not write a microarch name.

The governor reads RSS through `/proc/self/status` with a 100 ms cache. `memory_mode: hard` returns 503 over the cap. `degrade` sets `FLAG_DEGRADED`.

Shared hot atomics are line-padded (`#[repr(C, align(64))]`, 64-byte line). Workers on different cores do not false-share those counters.

`Sched::ip_key` hashes peer octets on the stack. It does not allocate.

Default response cache without a host file is 16 MiB. `compile.sh` writes a host overlay that sets `cache_bytes` to L3 size and `workers` to the physical core count.

Worker threads pin through FDS topology: worker `i` uses the first SMT sibling of physical core `i`. Two workers on a sibling pair share L1/L2.

Hot-path allocation is zero in the H1 engine after warm-up. Receive buffers, the connection table, and the encoder scratch are preallocated.

Measured numbers: [../Benchmarks.md](../Benchmarks.md). Those numbers are one machine. Run `./compile.sh` on the box you measure.

Compile procedure: [Compile.md](Compile.md).
