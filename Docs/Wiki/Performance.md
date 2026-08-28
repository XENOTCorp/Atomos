# Performance

Release profile: opt-level 3, thin LTO, codegen-units 1, panic abort, strip.

CPU flags come from `Code/scripts/cpu-rustflags.sh` (`target-cpu=native` plus features in `/proc/cpuinfo`).

The governor reads RSS through `/proc/self/status` with a 100 ms cache. `memory_mode: hard` returns 503 over the cap. `degrade` sets `FLAG_DEGRADED`.

Shared hot atomics are line-padded (`#[repr(C, align(64))]`, 64-byte line). Workers on different cores do not false-share those counters.

`Sched::ip_key` hashes peer octets on the stack. It does not allocate.

Default response cache without a host file is 16 MiB. The host overlay raises `cache_bytes` to L3 size.

Hot-path allocation is zero in the H1 engine after warm-up. Receive buffers, the connection table, and the encoder scratch are preallocated.

Measured numbers: [../Benchmarks.md](../Benchmarks.md).
