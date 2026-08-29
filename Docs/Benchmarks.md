# Benchmarks

`Code/bench/run.sh` builds the tree, runs Atomos H1 plaintext, nginx, h2o, and `atomos-proto` (TLS, h2c, HTTP/3) one server at a time, and writes `Code/bench/out/$DATE.json`. The script fails if Atomos H1 plaintext 11 B drops more than 15% versus `Code/bench/baseline.json`. Nightly runs on a self-hosted Linux box.

## Method

- Machine: Intel Core i5-5200U (2 physical cores, 4 logical), kernel 7.2.0_1 (Void Linux), loopback only. Governor `performance`.
- Atomos workers: one pinned epoll thread per physical core (2 on this host). Pinning uses FDS topology (first SMT sibling of each core: logical 0 and 2).
- Load: `wrk -t4 -c256 -d15s --latency` for HTTP/1.1. `h2load` for HTTP/2. In-repo `bench_h23` for HTTP/2 stream latency and HTTP/3.
- Payloads: 11 B, 64 KiB, 1 MiB. Same files for every server.
- nginx `open_file_cache`, `worker_processes 4`. h2o `num-threads: 4`. Each server is the only listener on the box for its rows.

## HTTP/1.1 throughput

`wrk -t4 -c256 -d15s --latency`, keep-alive.

| Server | 11 B | Rank | 64 KiB | Rank | 1 MiB | Rank |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| h2o 2.2.6 | 74,995 | 1 | 24,831 (1.63 GB/s) | 3 | 1,843 (1.93 GB/s) | 3 |
| Atomos H1 (epoll) | 62,775 | 2 | 29,423 (1.93 GB/s) | 1 | 3,390 (3.55 GB/s) | 1 |
| nginx 1.30.4 | 50,191 | 3 | 25,898 (1.70 GB/s) | 2 | 2,740 (2.87 GB/s) | 2 |

Atomos is first or second on every payload.

## HTTP/1.1 latency

Same wrk command. Percentiles from wrk `--latency`.

| Server | 11 B p50 | 11 B p99 | 64 KiB p50 | 64 KiB p99 | 1 MiB p50 | 1 MiB p99 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| h2o 2.2.6 | 2.60 ms | 15.2 ms | 7.91 ms | 27.5 ms | 117 ms | 378 ms |
| Atomos H1 (epoll) | 3.07 ms | 13.5 ms | 6.81 ms | 18.8 ms | 36.0 ms | 252 ms |
| nginx 1.30.4 | 4.36 ms | 16.0 ms | 6.06 ms | 29.7 ms | 40.8 ms | 584 ms |

## HTTP/2

`atomos-proto` h2c, 2 workers. h2o h2c on the same host, same payload.

| Load | Atomos | h2o |
| --- | ---: | ---: |
| `h2load -n50000 -c16 -m64` (mux) | 49,390 req/s | 98,260 req/s |
| `h2load -n20000 -c1 -m1` (seq) | 8,417 req/s | 9,677 req/s |
| `bench_h23` mux x64 (single connection) | 36,669 req/s | — |
| `bench_h23` seq (2000 reqs) | 4,281 req/s, p50 153 µs, p99 2.3 ms | — |

`atomos-proto` HTTPS HTTP/2, `h2load -n50000 -c16 -m64`: 45,700 req/s.

## HTTP/3

`atomos-proto` QUIC, `bench_h23`, 2000 requests:

| Mode | Throughput | p50 | p99 |
| --- | ---: | ---: | ---: |
| Sequential | 2,271 req/s | 322 µs | 2.5 ms |
| Mux x64 | 13,022 req/s | — | — |

## TLS

`atomos-proto` HTTPS, `wrk -t4 -c256 -d15s --latency`, 11 B: 31,832 req/s, p50 6.16 ms.

## Transport

The Atomos H1 engine is the FDS epoll transport with an HTTP state machine on top. Connection state is a slot array indexed by FDS `ConnectionId`. Header, body, and idle timeouts are enforced on a 200 ms cadence, not on every epoll event. The vendored FDS tree also contains the io_uring and AF_XDP datapaths; H1 does not use them.
