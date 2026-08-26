# Atomos benchmarks

Measured comparisons of the Atomos HTTP kernel against existing HTTP
servers. All measurements are apples-to-apples: the same machine, the
same load generator, the same payload files, the same connection
counts, and the same duration for every row of a table.

## Method

- Machine: Intel Core i5-5200U (2 physical cores, 4 logical), kernel
  7.2.0_1 (Void Linux), loopback only. This is the full hardware
  statement.
- Load generators: `wrk -t4 -c100 -d5s` for HTTP/1.1, `h2load` for
  HTTP/2, `curl` for latency percentiles, and the in-repo `bench_h23`
  client for HTTP/3.
- Payload files: `index.html` (11 bytes) and `file64k.bin` (64 KiB),
  identical for every server.
- Servers run out of the box except where a row states otherwise:
  nginx uses `open_file_cache` (tuned static path), h2o uses
  `num-threads: 4`, Seastar uses `--smp 4`, and the Rust servers use
  their default runtimes.
- Percentiles are reported as p10, p50, p90, p95, p99, p999.

## HTTP/1.1 throughput

`wrk -t4 -c100 -d5s`, keep-alive, cached page (11 bytes) and 64 KiB
file:

| Server | Cached page | 64 KiB file |
| --- | --- | --- |
| Atomos H1 (FDS epoll) | 100,328 req/s | 27,848 req/s (1.70 GB/s) |
| h2o 2.2.6 | 86,755 req/s | 26,791 req/s (1.64 GB/s) |
| nginx 1.30.4 (open_file_cache) | 70,379 req/s | 33,805 req/s (2.07 GB/s) |
| Hyper 1 (tokio) | 41,024 req/s | 18,688 req/s (1.14 GB/s) |
| Caddy 2.11.4 | 18,513 req/s | 15,194 req/s (0.93 GB/s) |
| Seastar httpd | 10,334 req/s | 2,412 req/s (151.5 MB/s) |
| axum 0.8 | 2,533 req/s | 9,751 req/s (611.2 MB/s) |
| actix-web 4 | 2,514 req/s | 15,633 req/s (959.3 MB/s) |

Notes:

- The cached page for Atomos, h2o, and nginx is an in-memory response
  path: Atomos and h2o use a wire cache, nginx uses `open_file_cache`
  on its static path. axum and Hyper read the file per request.
- On the 64 KiB file, nginx's sendfile leads; Atomos and h2o are at
  parity on the byte path.
- Seastar's httpd demo serves files through its directory handler
  (`/file/<name>`, mapped to the filesystem root) without a response
  cache.

## HTTP/1.1 latency percentiles

Fresh connection per request, `curl` total time, 200 samples:

| Server | p10 | p50 | p90 | p99 | p999 | mean |
| --- | --- | --- | --- | --- | --- | --- |
| Atomos H1 | 259 | 291 | 337 | 1484 | 2468 | 321.5 |
| nginx | 292 | 323 | 366 | 601 | 677 | 330.6 |
| h2o | 329 | 343 | 385 | 913 | 1091 | 362.1 |
| Seastar | 584 | 669 | 1302 | 2750 | 2772 | 835.5 |
| Hyper | 355 | 393 | 457 | 1004 | 1144 | 411.6 |
| axum | 444 | 498 | 593 | 1393 | 2011 | 523.9 |
| Caddy | 515 | 549 | 600 | 1471 | 1778 | 568.6 |

All values are microseconds.

## HTTP/2

`h2load`, 50,000 requests across 16 connections with 64 streams each
(multiplexed) and 20,000 requests on one connection (sequential):

| Server | Multiplexed | Sequential |
| --- | --- | --- |
| h2o 2.2.6 | 254,079 req/s | 18,332 req/s |
| Atomos (atomos-proto, h2c) | 166,761 req/s | 16,364 req/s |
| nghttpd (nghttp2) | not comparable | not comparable |

nghttpd returned 404 for `/` (no index handling) in every request, so
its rows are excluded. The Atomos H2 wire cost per request is 149 B
first and 12 B steady (HPACK static and dynamic table hits).

## HTTP/3

`atomos-proto` with the QUIC path, 1000 requests (in-repo `bench_h23`
client):

| Mode | Throughput | p50 | p99 |
| --- | --- | --- | --- |
| Sequential | 7,225 req/s | 123.4 µs | 427.0 µs |
| Multiplexed (64 streams) | 38,071 req/s | - | - |

A head-to-head HTTP/3 comparison against nghttpx or quiche was not
measurable on this platform: the `h2load` QUIC client and the
available servers are not interoperable on this machine. The HTTP/3
numbers above are the server's own measured behavior.

## Resource counters

`perf stat` on the Atomos H1 server under the `wrk` load, 4 seconds:
IPC 0.54, branch miss rate 1.20%, dTLB miss rate 0.19%, L1-dcache
misses 266.6 M, page faults 1,240, context switches about 6,100 per
second, zero CPU migrations. Peak heap at startup (massif): 2.73 MB.
CPU split under load (mpstat): %usr 23.6, %sys 56.7, %soft 19.1.

Time to first byte, fresh connection, 100 samples: p50 233 µs,
p99 2404 µs.

## Stacks not run on this platform

- msquic: requires a QUIC build environment and a certificate store.
- Netty: the netty-all artifact is not retrievable through this
  network; the JDK is present.
- quiche: requires BoringSSL, which needs NASM; NASM is not installed.
- DPDK and F-Stack: require a DPDK-capable NIC. The only live link here
  is Wi-Fi.

## Transport

The transport measurements (echo throughput, latency percentiles,
SCTP, reactor strategies) are in the FDS repository:
[BENCHMARKS.md](../FDS/BENCHMARKS.md). The Atomos H1 engine is the FDS
epoll transport with an HTTP state machine on top.
