# Overview

Atomos is an HTTP kernel in Rust. A consumer registers named modules and loads a disjoint JSON ruleset. The kernel accepts TCP, parses headers, scans JSON depth, matches the ruleset, runs the module, and writes the response.

Two engines share one kernel:

- `atomos` serves HTTP/1.1 on the FDS epoll transport.
- `atomos-proto` serves HTTP/1.1, HTTP/2, HTTP/3, and TLS on tokio.

FDS crates live in `Code/FDS`.
