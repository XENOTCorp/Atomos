# H-04 decision matrix (origin hot path)

Measured on Broadwell i5-5200U, loopback, pinned workers. Criticality C2.

| Quantity | Symbol | Value |
|---|---|---|
| Serial H1 p50 (C ping / Atomos) | `T_sys` | ~30 µs / ~29 µs |
| Pipelined kernel+parse+cache floor | `T_sys` | ≳ 18 µs |
| Cache-hit plugin work | | **0** (H-07) |
| Dominant hot-path case (static/health) | `p_hit` | → 1 |

`.so` / Lua extra vs native on the uncached path: `Δ = (1 − p_hit) · (T_w − T_n)`. As `p_hit → 1`, `gain = Δ / T_sys → 0`. **Refuse `kind: native` and Lua.**

| Candidate | Decision | Evidence |
|---|---|---|
| epoll RTC vs tokio spawn-per-conn H1 | **epoll** | H1 16×16 ~143k; tokio Future tax |
| HTTP/2 as fast path | **no** | H1 143k > h2 ~39k > h3 ~5.3k |
| simd-json | **no** | serde faster on 56 B |
| tokio-uring | **no** | tokio p99 better; `!Sync` |
| software prefetch | **no** | measured slower |
| io_uring in product | **no** | measured |
| pico C parser | **no** | L-02 C TCB; try Rust SIMD only if it beats httparse |
| kTLS | **no** | origin H1 is loopback; extra OpenSSL TCB |
| shm slab cache | **no** | CC-02; 2 workers, RSS bound already |
| AF_XDP on this host | **no** | loopback cannot |
| Wasm on cache hit | **no** | H-07 |
| busy-poll always | **no** | helps idle→busy p50, not saturated rps; extra CPU/timing surface |
