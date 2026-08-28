# Overview

Atomos is an HTTP kernel in Rust. A consumer registers named modules and loads a disjoint JSON ruleset. The kernel accepts TCP, parses headers, scans JSON depth, matches the ruleset, runs the module, and writes the response.

The kernel does not know about any product. It is not a framework. It is not a product server. It provides the request path. The application provides the modules.

Two engines share one kernel:

- `atomos` serves HTTP/1.1 on the FDS epoll transport.
- `atomos-proto` serves HTTP/1.1, HTTP/2, HTTP/3, and TLS on tokio.

FDS crates live in `Code/FDS`. You do not clone FDS to build Atomos.
