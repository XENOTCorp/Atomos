#!/usr/bin/env python3
"""Loopback load test for ATOMOS first_app. No extra deps.

Measures per-endpoint latency (avg/median/stdev/p10/p30/p50/p90/p95/p99),
requests/sec, HTTP errors, and samples the server's RSS + CPU from /proc.

  python3 scripts/loadtest.py --bind 127.0.0.1:8090 --pid 1234
"""
from __future__ import annotations

import argparse
import http.client
import json
import os
import statistics
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass, field
from typing import Callable, Optional


CLK = os.sysconf("SC_CLK_TCK")
NPROC = os.cpu_count() or 1


def read_status(pid: int) -> dict:
    out = {
        "rss_kib": None,
        "hwm_kib": None,
        "size_kib": None,
        "threads": None,
        "fdsize": None,
    }
    try:
        with open(f"/proc/{pid}/status", encoding="utf-8") as f:
            for line in f:
                if line.startswith("VmRSS:"):
                    out["rss_kib"] = int(line.split()[1])
                elif line.startswith("VmHWM:"):
                    out["hwm_kib"] = int(line.split()[1])
                elif line.startswith("VmSize:"):
                    out["size_kib"] = int(line.split()[1])
                elif line.startswith("Threads:"):
                    out["threads"] = int(line.split()[1])
                elif line.startswith("FDSize:"):
                    out["fdsize"] = int(line.split()[1])
    except FileNotFoundError:
        pass
    return out


def read_cpu_ticks(pid: int) -> Optional[int]:
    try:
        with open(f"/proc/{pid}/stat", encoding="utf-8") as f:
            st = f.read().split()
        # utime=13, stime=14 (0-indexed after comm)
        return int(st[13]) + int(st[14])
    except (FileNotFoundError, IndexError, ValueError):
        return None


class Sampler:
    def __init__(self, pid: int, interval_s: float = 0.05):
        self.pid = pid
        self.interval_s = interval_s
        self.samples: list[dict] = []
        self._stop = threading.Event()
        self._t: Optional[threading.Thread] = None

    def start(self) -> None:
        self._stop.clear()
        self.samples = []
        self._t = threading.Thread(target=self._run, daemon=True)
        self._t.start()

    def _run(self) -> None:
        t0 = time.perf_counter()
        cpu0 = read_cpu_ticks(self.pid)
        while not self._stop.is_set():
            st = read_status(self.pid)
            now = time.perf_counter()
            cpu = read_cpu_ticks(self.pid)
            cpu_pct = None
            if cpu is not None and cpu0 is not None and now > t0:
                cpu_pct = 100.0 * ((cpu - cpu0) / CLK) / (now - t0) / NPROC
            self.samples.append(
                {
                    "t_s": round(now - t0, 4),
                    "rss_kib": st["rss_kib"],
                    "hwm_kib": st["hwm_kib"],
                    "size_kib": st["size_kib"],
                    "threads": st["threads"],
                    "cpu_pct_of_machine": None
                    if cpu_pct is None
                    else round(cpu_pct, 2),
                }
            )
            self._stop.wait(self.interval_s)

    def stop(self) -> dict:
        self._stop.set()
        if self._t is not None:
            self._t.join(timeout=2)
        rss = [s["rss_kib"] for s in self.samples if s["rss_kib"] is not None]
        hwm = [s["hwm_kib"] for s in self.samples if s["hwm_kib"] is not None]
        cpu = [
            s["cpu_pct_of_machine"]
            for s in self.samples
            if s["cpu_pct_of_machine"] is not None
        ]
        return {
            "n_samples": len(self.samples),
            "rss_kib_min": min(rss) if rss else None,
            "rss_kib_max": max(rss) if rss else None,
            "rss_kib_last": rss[-1] if rss else None,
            "rss_kib_avg": round(sum(rss) / len(rss), 1) if rss else None,
            "hwm_kib_peak": max(hwm) if hwm else None,
            "cpu_pct_avg": round(sum(cpu) / len(cpu), 2) if cpu else None,
            "cpu_pct_max": max(cpu) if cpu else None,
            "threads_last": self.samples[-1]["threads"] if self.samples else None,
            "series": self.samples,
        }


def pct(sorted_ms: list[float], p: float) -> float:
    if not sorted_ms:
        return float("nan")
    n = len(sorted_ms)
    i = int(round((p / 100.0) * (n - 1)))
    i = max(0, min(n - 1, i))
    return sorted_ms[i]


def summarize(ms: list[float], n_ok: int, n_err: int, wall_s: float, bytes_n: int) -> dict:
    srt = sorted(ms)
    n = len(srt)
    mean = statistics.fmean(srt) if n else float("nan")
    med = statistics.median(srt) if n else float("nan")
    stdev = statistics.pstdev(srt) if n > 1 else 0.0
    rps = (n_ok + n_err) / wall_s if wall_s > 0 else float("nan")
    return {
        "n": n,
        "ok": n_ok,
        "err": n_err,
        "wall_s": round(wall_s, 4),
        "rps": round(rps, 1),
        "bytes": bytes_n,
        "avg_ms": round(mean, 3),
        "median_ms": round(med, 3),
        "stdev_ms": round(stdev, 3),
        "min_ms": round(srt[0], 3) if n else None,
        "max_ms": round(srt[-1], 3) if n else None,
        "p10_ms": round(pct(srt, 10), 3) if n else None,
        "p30_ms": round(pct(srt, 30), 3) if n else None,
        "p50_ms": round(pct(srt, 50), 3) if n else None,
        "p90_ms": round(pct(srt, 90), 3) if n else None,
        "p95_ms": round(pct(srt, 95), 3) if n else None,
        "p99_ms": round(pct(srt, 99), 3) if n else None,
    }


@dataclass
class Target:
    method: str
    path: str
    body: Optional[bytes] = None
    headers: dict = field(default_factory=dict)


def one(conn: http.client.HTTPConnection, t: Target) -> tuple[float, int, int]:
    hdrs = dict(t.headers)
    if t.body is not None and "Content-Type" not in hdrs:
        hdrs["Content-Type"] = "application/json"
    t0 = time.perf_counter()
    conn.request(t.method, t.path, body=t.body, headers=hdrs)
    resp = conn.getresponse()
    data = resp.read()
    dt = (time.perf_counter() - t0) * 1000.0
    return dt, resp.status, len(data)


def run_serial(host: str, port: int, target: Target, n: int, warmup: int) -> dict:
    conn = http.client.HTTPConnection(host, port, timeout=10)
    try:
        for _ in range(warmup):
            one(conn, target)
        ms: list[float] = []
        n_ok = n_err = 0
        nbytes = 0
        t0 = time.perf_counter()
        for _ in range(n):
            dt, status, blen = one(conn, target)
            ms.append(dt)
            nbytes += blen
            if 200 <= status < 400:
                n_ok += 1
            else:
                n_err += 1
        wall = time.perf_counter() - t0
        return summarize(ms, n_ok, n_err, wall, nbytes)
    finally:
        conn.close()


def run_conc(
    host: str, port: int, target: Target, n: int, workers: int, warmup: int
) -> dict:
    # Warm one connection so the listener is hot.
    c = http.client.HTTPConnection(host, port, timeout=10)
    try:
        for _ in range(warmup):
            one(c, target)
    finally:
        c.close()

    lock = threading.Lock()
    ms: list[float] = []
    n_ok = n_err = 0
    nbytes = 0

    def worker(k: int) -> None:
        nonlocal n_ok, n_err, nbytes
        conn = http.client.HTTPConnection(host, port, timeout=10)
        local_ms: list[float] = []
        lok = lerr = lb = 0
        try:
            for _ in range(k):
                dt, status, blen = one(conn, target)
                local_ms.append(dt)
                lb += blen
                if 200 <= status < 400:
                    lok += 1
                else:
                    lerr += 1
        finally:
            conn.close()
        with lock:
            ms.extend(local_ms)
            n_ok += lok
            n_err += lerr
            nbytes += lb

    per = n // workers
    extra = n % workers
    t0 = time.perf_counter()
    with ThreadPoolExecutor(max_workers=workers) as ex:
        futs = []
        for i in range(workers):
            k = per + (1 if i < extra else 0)
            futs.append(ex.submit(worker, k))
        for f in as_completed(futs):
            f.result()
    wall = time.perf_counter() - t0
    return summarize(ms, n_ok, n_err, wall, nbytes)


def run_mixed(
    host: str, port: int, n: int, workers: int
) -> dict:
    seq = [
        Target("GET", "/"),
        Target("GET", "/api/health"),
        Target("GET", "/api/notes"),
    ]

    def pick(i: int) -> Target:
        return seq[i % len(seq)]

    lock = threading.Lock()
    ms: list[float] = []
    n_ok = n_err = 0
    nbytes = 0

    def worker(start: int, k: int) -> None:
        nonlocal n_ok, n_err, nbytes
        conn = http.client.HTTPConnection(host, port, timeout=10)
        local: list[float] = []
        lok = lerr = lb = 0
        try:
            for j in range(k):
                dt, status, blen = one(conn, pick(start + j))
                local.append(dt)
                lb += blen
                if 200 <= status < 400:
                    lok += 1
                else:
                    lerr += 1
        finally:
            conn.close()
        with lock:
            ms.extend(local)
            n_ok += lok
            n_err += lerr
            nbytes += lb

    per = n // workers
    extra = n % workers
    t0 = time.perf_counter()
    with ThreadPoolExecutor(max_workers=workers) as ex:
        futs = []
        off = 0
        for i in range(workers):
            k = per + (1 if i < extra else 0)
            futs.append(ex.submit(worker, off, k))
            off += k
        for f in as_completed(futs):
            f.result()
    wall = time.perf_counter() - t0
    return summarize(ms, n_ok, n_err, wall, nbytes)


def run_post(host: str, port: int, n: int) -> dict:
    conn = http.client.HTTPConnection(host, port, timeout=10)
    try:
        ms: list[float] = []
        n_ok = n_err = 0
        nbytes = 0
        t0 = time.perf_counter()
        for i in range(n):
            body = json.dumps({"text": f"bench-{i}"}).encode()
            t = Target("POST", "/api/notes", body=body)
            dt, status, blen = one(conn, t)
            ms.append(dt)
            nbytes += blen
            if 200 <= status < 400:
                n_ok += 1
            else:
                n_err += 1
        wall = time.perf_counter() - t0
        return summarize(ms, n_ok, n_err, wall, nbytes)
    finally:
        conn.close()


def row(name: str, s: dict) -> str:
    return (
        f"| {name} | {s['n']} | {s['ok']} | {s['err']} | {s['rps']:.0f} | "
        f"{s['avg_ms']:.3f} | {s['median_ms']:.3f} | {s['stdev_ms']:.3f} | "
        f"{s['p10_ms']:.3f} | {s['p30_ms']:.3f} | {s['p50_ms']:.3f} | "
        f"{s['p90_ms']:.3f} | {s['p95_ms']:.3f} | {s['p99_ms']:.3f} | "
        f"{s['min_ms']:.3f} | {s['max_ms']:.3f} |"
    )


def mem_row(name: str, m: dict) -> str:
    def kib(x):
        return "—" if x is None else f"{x/1024:.2f}"

    return (
        f"| {name} | {m.get('n_samples')} | {kib(m.get('rss_kib_min'))} | "
        f"{kib(m.get('rss_kib_avg'))} | {kib(m.get('rss_kib_max'))} | "
        f"{kib(m.get('hwm_kib_peak'))} | {m.get('cpu_pct_avg')} | "
        f"{m.get('cpu_pct_max')} | {m.get('threads_last')} |"
    )


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--bind", default="127.0.0.1:8090")
    ap.add_argument("--pid", type=int, required=True)
    ap.add_argument("--n", type=int, default=4000)
    ap.add_argument("--warmup", type=int, default=100)
    ap.add_argument("--workers", type=int, default=4)
    ap.add_argument("--post-n", type=int, default=200)
    ap.add_argument("--out", default="-")
    args = ap.parse_args()
    host, port_s = args.bind.rsplit(":", 1)
    port = int(port_s)

    report: dict = {
        "bind": args.bind,
        "pid": args.pid,
        "nproc": NPROC,
        "n": args.n,
        "workers": args.workers,
        "scenarios": {},
        "resources": {},
    }

    idle = Sampler(args.pid, 0.05)
    idle.start()
    time.sleep(1.0)
    report["resources"]["idle"] = idle.stop()

    scenarios: list[tuple[str, Callable[[], dict]]] = [
        (
            "GET /  keepalive n=%d" % args.n,
            lambda: run_serial(host, port, Target("GET", "/"), args.n, args.warmup),
        ),
        (
            "GET /api/health  keepalive n=%d (1s cache)" % args.n,
            lambda: run_serial(
                host, port, Target("GET", "/api/health"), args.n, args.warmup
            ),
        ),
        (
            "GET /api/notes  keepalive n=%d" % args.n,
            lambda: run_serial(
                host, port, Target("GET", "/api/notes"), args.n, args.warmup
            ),
        ),
        (
            "GET /api/health  %d workers n=%d" % (args.workers, args.n),
            lambda: run_conc(
                host,
                port,
                Target("GET", "/api/health"),
                args.n,
                args.workers,
                args.warmup,
            ),
        ),
        (
            "GET /  %d workers n=%d" % (args.workers, args.n),
            lambda: run_conc(
                host, port, Target("GET", "/"), args.n, args.workers, args.warmup
            ),
        ),
        (
            "GET /api/notes  %d workers n=%d" % (args.workers, args.n),
            lambda: run_conc(
                host,
                port,
                Target("GET", "/api/notes"),
                args.n,
                args.workers,
                args.warmup,
            ),
        ),
        (
            "mixed GET / + /api/health + /api/notes  %d workers n=%d"
            % (args.workers, args.n),
            lambda: run_mixed(host, port, args.n, args.workers),
        ),
        (
            "POST /api/notes  keepalive n=%d" % args.post_n,
            lambda: run_post(host, port, args.post_n),
        ),
        (
            "GET /api/notes after POST  keepalive n=%d" % (args.n // 2),
            lambda: run_serial(
                host, port, Target("GET", "/api/notes"), args.n // 2, 20
            ),
        ),
        (
            "GET /api/health  8 workers n=%d" % args.n,
            lambda: run_conc(
                host, port, Target("GET", "/api/health"), args.n, 8, args.warmup
            ),
        ),
    ]

    md_lat = [
        "| scenario | n | ok | err | rps | avg | median | stdev | p10 | p30 | p50 | p90 | p95 | p99 | min | max |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    md_mem = [
        "| situation | samples | RSS min MiB | RSS avg MiB | RSS max MiB | HWM peak MiB | CPU avg % of machine | CPU max % | threads |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|",
        mem_row("idle 1s", report["resources"]["idle"]),
    ]

    for name, fn in scenarios:
        samp = Sampler(args.pid, 0.05)
        samp.start()
        stats = fn()
        mem = samp.stop()
        report["scenarios"][name] = {"latency": stats, "resource": {k: v for k, v in mem.items() if k != "series"}}
        report["resources"][name] = {k: v for k, v in mem.items() if k != "series"}
        # keep series only for mixed + idle to bound file size
        if "mixed" in name:
            report["resources"][name]["series"] = mem["series"]
        md_lat.append(row(name, stats))
        md_mem.append(mem_row(name, mem))
        print(name, file=sys.stderr)
        print(
            "  rps={rps:.0f} avg={avg_ms:.3f}ms p50={p50_ms:.3f} p99={p99_ms:.3f} err={err}".format(
                **stats
            ),
            file=sys.stderr,
        )
        print(
            "  rss_avg={rss_kib_avg}KiB hwm={hwm_kib_peak}KiB cpu_avg={cpu_pct_avg}%".format(
                **{k: mem.get(k) for k in ("rss_kib_avg", "hwm_kib_peak", "cpu_pct_avg")}
            ),
            file=sys.stderr,
        )

    cooldown = Sampler(args.pid, 0.05)
    cooldown.start()
    time.sleep(1.0)
    report["resources"]["cooldown"] = cooldown.stop()
    md_mem.append(mem_row("cooldown 1s", report["resources"]["cooldown"]))

    text = "\n".join(
        [
            "## Latency (milliseconds, loopback HTTP/1.1 keep-alive unless noted)",
            "",
            *md_lat,
            "",
            "## Memory and CPU (from `/proc/<pid>/status` + `stat`)",
            "",
            *md_mem,
            "",
        ]
    )
    payload = {"markdown": text, "json": report}
    if args.out == "-":
        sys.stdout.write(text)
        sys.stdout.write("\n")
        json.dump(report, sys.stdout, indent=2)
        sys.stdout.write("\n")
    else:
        with open(args.out, "w", encoding="utf-8") as f:
            f.write(text)
            f.write("\n```json\n")
            json.dump(report, f, indent=2)
            f.write("\n```\n")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BrokenPipeError:
        raise SystemExit(0)
