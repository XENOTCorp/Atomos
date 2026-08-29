# Benchmarks

Reproducible numbers live in `Code/bench/`. `Code/bench/run.sh` builds the tree, runs Atomos H1 plaintext, Atomos proto TLS, nginx, and h2o one at a time, writes `Code/bench/out/$DATE.json`, and fails if Atomos H1 plaintext 11 B drops more than 15% versus `Code/bench/baseline.json`. GitHub-hosted Ubuntu is not the source of truth. Nightly is a self-hosted Linux box.

The isolated SOTA benches from the H1 4 KiB accept-path work are produced by that harness (or they are out of the story). This file is the last laptop run.

## Last laptop run

Measured on one machine, one load generator, one payload set. Each server is the only listener on the box for its rows. Seastar `--smp 4` busy-polls idle cores; it is killed before the next server starts.

## Method

- Machine: Intel Core i5-5200U (2 physical cores, 4 logical), kernel 7.2.0_1 (Void Linux), loopback only.
- Load generators: `wrk -t4 -c100 -d5s` for HTTP/1.1, `h2load` for HTTP/2, `curl` for latency percentiles, in-repo `bench_h23` for HTTP/3.
- Payloads: `index.html` (11 bytes), `file64k.bin` (64 KiB), `file1m.bin` (1 MiB). Same files for every server.
- Servers: nginx `open_file_cache`, h2o `num-threads: 4`, Seastar `--smp 4`, Rust servers on their default runtimes. Atomos: 4 pinned FDS epoll workers, wire cache, host overlay `cache_bytes` = L3.
- Percentiles: p10, p50, p90, p95, p99, p999.

## HTTP/1.1 throughput

`wrk -t4 -c100 -d5s`, keep-alive, cached page (11 bytes) and 64 KiB file:

| Server | Cached page | Rank | % vs Atomos | 64 KiB file |
| --- | --- | --- | --- | --- |
| Atomos H1 (FDS epoll) | 86,032 req/s | 1 | 0% (baseline) | 26,778 req/s (1.64 GB/s) |
| h2o 2.2.6 | 84,538 req/s | 2 | -1.7% | 26,768 req/s (1.64 GB/s) |
| nginx 1.30.4 (open_file_cache) | 65,522 req/s | 3 | -23.8% | 33,875 req/s (2.08 GB/s) |
| Hyper 1 (tokio) | 41,923 req/s | 4 | -51.3% | 18,891 req/s (1.15 GB/s) |
| Caddy 2.11.4 | 18,642 req/s | 5 | -78.3% | 15,200 req/s (0.93 GB/s) |
| Seastar httpd | 6,628 req/s | 6 | -92.3% | 2,289 req/s (143.7 MB/s) |
| axum 0.8 | 2,529 req/s | 7 | -97.1% | 9,752 req/s (611.2 MB/s) |
| actix-web 4 | 2,507 req/s | 8 | -97.1% | 15,348 req/s (0.94 GB/s) |

64 KiB file ranking:

| Server | 64 KiB file | Rank | % vs Atomos |
| --- | --- | --- | --- |
| nginx 1.30.4 (open_file_cache) | 33,875 req/s (2.08 GB/s) | 1 | +26.5% |
| Atomos H1 (FDS epoll) | 26,778 req/s (1.64 GB/s) | 2 | 0% (baseline) |
| h2o 2.2.6 | 26,768 req/s (1.64 GB/s) | 3 | 0.0% |
| Hyper 1 (tokio) | 18,891 req/s (1.15 GB/s) | 4 | -29.5% |
| actix-web 4 | 15,348 req/s (0.94 GB/s) | 5 | -42.7% |
| Caddy 2.11.4 | 15,200 req/s (0.93 GB/s) | 6 | -43.2% |
| axum 0.8 | 9,752 req/s (611.2 MB/s) | 7 | -63.6% |
| Seastar httpd | 2,289 req/s (143.7 MB/s) | 8 | -91.5% |

## 1 MiB file

`wrk -t8 -c512 -d10s -T 30s`, keep-alive. Concurrency swept on nginx from 100 to 2,048; loopback peaks at 512 on this box.

| Server | 1 MiB file | Rank | % vs Atomos |
| --- | --- | --- | --- |
| nginx 1.30.4 (open_file_cache) | 3,571 req/s (3.49 GB/s) | 1 | +9.0% |
| Atomos H1 (FDS epoll) | 3,277 req/s (3.20 GB/s) | 2 | 0% (baseline) |
| Caddy 2.11.4 | 3,155 req/s (3.08 GB/s) | 3 | -3.7% |
| h2o 2.2.6 | 2,412 req/s (2.36 GB/s) | 4 | -26.4% |
| Hyper 1 (tokio) | 1,858 req/s (1.81 GB/s) | 5 | -43.3% |
| actix-web 4 | 1,241 req/s (1.23 GB/s) | 6 | -62.1% |
| axum 0.8 | 1,232 req/s (1.22 GB/s) | 7 | -62.4% |
| Seastar httpd | 189 req/s (209.8 MB/s) | 8 | -94.2% |

Atomos, h2o, and nginx serve the cached page from an in-memory path (wire cache or `open_file_cache`). axum and Hyper read the file per request. On 64 KiB and 1 MiB, nginx sendfile leads; Atomos and h2o match on the 64 KiB byte path. Seastar httpd serves through its directory handler (`/file/...`) with no response cache.

## HTTP/1.1 latency percentiles

Fresh connection per request, `curl` total time, 200 samples, microseconds. Ranked by p50.

| Server | p10 | p50 | p90 | p95 | p99 | p999 | mean | Rank (p50) | % vs Atomos (p50) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Atomos H1 | 259 | 292 | 355 | 451 | 1277 | 1519 | 322.8 | 1 | 0% (baseline) |
| nginx | 288 | 319 | 360 | 376 | 875 | 1466 | 326.6 | 2 | +9.2% |
| h2o | 329 | 341 | 380 | 445 | 879 | 990 | 358.6 | 3 | +16.8% |
| Hyper | 354 | 397 | 457 | 495 | 1193 | 1201 | 414.2 | 4 | +36.0% |
| axum | 477 | 533 | 643 | 747 | 1754 | 2920 | 579.3 | 5 | +82.5% |
| Caddy | 530 | 565 | 629 | 650 | 1500 | 2051 | 586.8 | 6 | +93.5% |
| Seastar | 602 | 641 | 885 | 1286 | 27638 | 74259 | 1244.0 | 7 | +119.5% |

## HTTP/2

`h2load`, 50,000 requests across 16 connections with 64 streams each (multiplexed) and 20,000 requests on one connection (sequential):

| Server | Multiplexed | Sequential | Rank | % vs Atomos (mux) |
| --- | --- | --- | --- | --- |
| h2o 2.2.6 | 233,905 req/s | 17,015 req/s | 1 | +93.4% |
| Atomos (atomos-proto, h2c) | 120,929 req/s | 15,970 req/s | 2 | 0% (baseline) |

## HTTP/3

`atomos-proto` QUIC path, 1000 requests, in-repo `bench_h23`:

| Mode | Throughput | p50 | p99 |
| --- | --- | --- | --- |
| Sequential | 6,936 req/s | 122.0 µs | 491.6 µs |
| Multiplexed (64 streams) | 32,179 req/s | - | - |

Multiplexing gain: 32,179 / 6,936 = 4.6x.

## Resource counters

`perf stat` on the Atomos H1 process under `wrk`, 4 seconds: IPC 0.57, branch miss rate 1.29%, dTLB miss rate 0.16%, L1-dcache misses 255.3 M, page faults 2,019, context switches about 7,550 per second, zero CPU migrations. Peak heap at startup (massif): 2.91 MB. CPU split under load (mpstat): %usr 21.7, %sys 57.3, %soft 19.5.

Time to first byte, fresh connection, 100 samples: p50 259 µs, p99 908 µs.

## Transport

The Atomos H1 engine is the FDS epoll transport with an HTTP state machine on top. Connection state is a slot array indexed by FDS `ConnectionId`. Transport measurements (echo throughput, latency percentiles, SCTP, reactor strategies) live in the FDS project.
