# Benchmarks

`Code/bench/run.sh` builds the tree, runs Atomos H1 plaintext, Atomos proto TLS, nginx, and h2o one server at a time, and writes `Code/bench/out/$DATE.json`. The script fails if Atomos H1 plaintext 11 B drops more than 15% versus `Code/bench/baseline.json`. Nightly runs on a self-hosted Linux box.

## Method

- Machine: Intel Core i5-5200U (2 physical cores, 4 logical), kernel 7.2.0_1 (Void Linux), loopback only.
- Atomos workers: one pinned epoll thread per physical core (2 on this host). Logical SMT siblings are not given their own workers.
- Load: `wrk -t4 -c256 -d15s --latency` for HTTP/1.1 (the in-tree harness). `h2load` for HTTP/2. In-repo `bench_h23` for HTTP/3.
- Payloads: 11 B, 64 KiB, 1 MiB. Same files for every server.
- nginx `open_file_cache`, h2o `num-threads: 4`. Each server is the only listener on the box for its rows.

## HTTP/1.1 throughput

`wrk -t4 -c256 -d15s`, keep-alive. Numbers below are from this host; each run writes `Code/bench/out/$DATE.json`.

| Server | 11 B | Rank | 64 KiB | Rank | 1 MiB | Rank |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| h2o 2.2.6 | 61,458 | 1 | 18,732 (0.88 GB/s) | 3 | 1,772 (1.37 GB/s) | 3 |
| Atomos H1 (epoll) | 53,477 | 2 | 22,441 (1.09 GB/s) | 2 | 2,612 (2.09 GB/s) | 1 |
| nginx 1.30.4 | 45,872 | 3 | 22,834 (1.26 GB/s) | 1 | 2,597 (2.21 GB/s) | 2 |

Atomos is first or second on every payload.

## HTTP/2

`atomos-proto` h2c, 2 workers.

| Load | Throughput |
| --- | ---: |
| `h2load -n50000 -c16 -m64` (mux) | 77,034 req/s |
| `h2load -n20000 -c1 -m1` (seq) | 11,231 req/s |
| `bench_h23` mux x64 (single connection) | 45,460 req/s |
| `bench_h23` seq (2000 reqs) | 7,919 req/s, p50 94 µs |

## HTTP/3

`atomos-proto` QUIC, `bench_h23`, 2000 requests:

| Mode | Throughput | p50 | p99 |
| --- | ---: | ---: | ---: |
| Sequential | 4,696 req/s | 161 µs | 1.8 ms |
| Mux x64 | 19,231 req/s | — | — |

## TLS

`atomos-proto` HTTPS, `wrk -t4 -c256 -d15s`, 11 B: 35,266 req/s.

## Transport

The Atomos H1 engine is the FDS epoll transport with an HTTP state machine on top. Connection state is a slot array indexed by FDS `ConnectionId`. Header, body, and idle timeouts are enforced on a 200 ms cadence, not on every epoll event.
