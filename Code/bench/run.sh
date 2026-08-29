#!/usr/bin/env bash
# One-at-a-time wrk / h2load / bench_h23 harness. Nightly self-hosted Linux only.
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
CODE="$ROOT/Code"
BENCH="$CODE/bench"
PAY="$BENCH/payloads"
OUT="$BENCH/out"
DATE=$(date -u +%Y%m%dT%H%M%SZ)
mkdir -p "$OUT" /tmp/atomos-nginx-body /tmp/atomos-nginx-proxy /tmp/atomos-nginx-fcgi /tmp/atomos-nginx-uwsgi /tmp/atomos-nginx-scgi

need() { command -v "$1" >/dev/null || { echo "missing $1" >&2; exit 1; }; }
need wrk
need nginx
need h2o
need python3

echo "== compile"
if [[ "${SKIP_COMPILE:-}" == 1 ]]; then
  echo "SKIP_COMPILE=1"
else
  (cd "$ROOT" && ./compile.sh)
  sleep "${BENCH_COOLDOWN_SECS:-30}"
fi
if [[ ! -x $CODE/target/release/examples/bench_h23 ]]; then
  (cd "$CODE" && cargo build --release --example bench_h23)
fi

BIN="$CODE/target/release/atomos"
PROTO="$CODE/target/release/atomos-proto"
H23="$CODE/target/release/examples/bench_h23"
if [[ ! -x $BIN ]]; then
  echo "missing $BIN" >&2
  exit 1
fi
if [[ ! -x $PROTO ]]; then
  echo "missing $PROTO" >&2
  exit 1
fi

TMP=$(mktemp -d)
trap 'kill $(jobs -p) 2>/dev/null || true; nginx -e /tmp/atomos-bench-nginx.err -c "$TMP/nginx.conf" -s stop 2>/dev/null || true; killall h2o 2>/dev/null || true; rm -rf "$TMP"' EXIT
mkdir -p "$TMP/www"
cp -a "$PAY/." "$TMP/www/"
ln -sfn 11b "$TMP/www/index.html" 2>/dev/null || cp "$PAY/11b" "$TMP/www/index.html"

# wrk stdout -> JSON: rps plus latency (microseconds).
wrk_json() {
  local url=$1
  local f
  f=$(mktemp "$TMP/wrk.XXXXXX")
  set +e
  wrk -t4 -c256 -d15s --latency "$url" >"$f" 2>&1
  set -e
  python3 - "$f" <<'PY'
import json, sys, re
text = open(sys.argv[1], encoding="utf-8", errors="replace").read()

def to_us(s):
    s = s.strip().lower()
    if s.endswith("us"):
        return float(s[:-2])
    if s.endswith("ms"):
        return float(s[:-2]) * 1000.0
    if s.endswith("s"):
        return float(s[:-1]) * 1_000_000.0
    return float(s)

out = {"rps": 0.0, "p50_us": 0.0, "p75_us": 0.0, "p90_us": 0.0, "p99_us": 0.0, "avg_us": 0.0, "max_us": 0.0}
for line in text.splitlines():
    if "Requests/sec" in line:
        try:
            out["rps"] = float(line.split()[-1])
        except ValueError:
            pass
    m = re.match(r"\s*(50|75|90|99)%\s+(\S+)", line)
    if m:
        out[f"p{m.group(1)}_us"] = to_us(m.group(2))
    parts = line.split()
    if parts and parts[0] == "Latency" and len(parts) >= 4:
        try:
            out["avg_us"] = to_us(parts[1])
            out["max_us"] = to_us(parts[3])
        except ValueError:
            pass
print(json.dumps(out, separators=(",", ":")))
PY
}

h2load_json() {
  local extra=()
  while [[ $# -gt 1 ]]; do
    extra+=("$1")
    shift
  done
  local url=$1
  local f
  f=$(mktemp "$TMP/h2load.XXXXXX")
  set +e
  h2load "${extra[@]}" "$url" >"$f" 2>&1
  set -e
  python3 - "$f" <<'PY'
import json, sys, re
text = open(sys.argv[1], encoding="utf-8", errors="replace").read()
out = {"rps": 0.0, "succeeded": 0, "failed": 0, "mean_us": 0.0, "max_us": 0.0}

def to_us(s):
    s = s.strip().lower()
    if s.endswith("us"):
        return float(s[:-2])
    if s.endswith("ms"):
        return float(s[:-2]) * 1000.0
    if s.endswith("s"):
        return float(s[:-1]) * 1_000_000.0
    return float(s)

m = re.search(r"([\d.]+)\s*req/s", text)
if m:
    out["rps"] = float(m.group(1))
m = re.search(r"requests:.*?(\d+)\s+succeeded.*?(\d+)\s+failed", text, re.S)
if m:
    out["succeeded"] = int(m.group(1))
    out["failed"] = int(m.group(2))
# time for request: min max mean sd
lines = text.splitlines()
for i, line in enumerate(lines):
    if "time for request" in line.lower() and i + 1 < len(lines):
        parts = lines[i + 1].split()
        if len(parts) >= 3:
            try:
                out["max_us"] = to_us(parts[1])
                out["mean_us"] = to_us(parts[2])
            except ValueError:
                pass
        break
print(json.dumps(out, separators=(",", ":")))
PY
}

kill_port() {
  local p=$1
  if command -v fuser >/dev/null 2>&1; then
    fuser -k "${p}/tcp" 2>/dev/null || true
    fuser -k "${p}/udp" 2>/dev/null || true
  else
    local pids
    pids=$(ss -H -ltnup "sport = :$p" 2>/dev/null | grep -oE 'pid=[0-9]+' | cut -d= -f2 | sort -u || true)
    if [[ -n "${pids:-}" ]]; then
      kill $pids 2>/dev/null || true
    fi
  fi
  sleep 0.3
}

# Physical cores: SMT siblings contend on the cached GET path.
WORKERS=$(awk '/^cpu cores/{print $4; exit}' /proc/cpuinfo)
SOCKS=$(awk '/^physical id/{print $4}' /proc/cpuinfo | sort -u | wc -l)
if [[ -n "$WORKERS" && "$WORKERS" -gt 0 && "$SOCKS" -gt 0 ]]; then
  WORKERS=$((WORKERS * SOCKS))
else
  WORKERS=$(nproc)
fi
if [[ -f "$CODE/.atomos/host.json" ]]; then
  export ATOMOS_HOST="$CODE/.atomos/host.json"
fi

cat >"$TMP/rules.json" <<'EOF'
{"rules":[{"id":"s","module":"static","methods":["GET","HEAD"],"include":["/*"],"exclude":[]}]}
EOF

echo "== atomos H1 plaintext"
kill_port 18090
cat >"$TMP/atomos-h1.json" <<EOF
{"bind":"127.0.0.1:18090","static_root":"$TMP/www","memory_cap_bytes":67108864,"engine":"epoll","workers":$WORKERS,"http2":false,"http3":false,"allow_non_loopback":false}
EOF
"$BIN" --config "$TMP/atomos-h1.json" --rules "$TMP/rules.json" >/tmp/atomos-bench-h1.log 2>&1 &
sleep 0.5
A11=$(wrk_json http://127.0.0.1:18090/11b)
A64=$(wrk_json http://127.0.0.1:18090/64k)
A1M=$(wrk_json http://127.0.0.1:18090/1m)
echo "atomos H1 11b $A11"
echo "atomos H1 64k $A64"
echo "atomos H1 1m  $A1M"
kill_port 18090

echo "== nginx"
kill_port 18092
sed "s|ROOT|$TMP/www|g" "$BENCH/nginx.conf" >"$TMP/nginx.conf"
nginx -e /tmp/atomos-bench-nginx.err -c "$TMP/nginx.conf"
sleep 0.3
N11=$(wrk_json http://127.0.0.1:18092/11b)
N64=$(wrk_json http://127.0.0.1:18092/64k)
N1M=$(wrk_json http://127.0.0.1:18092/1m)
echo "nginx 11b $N11"
echo "nginx 64k $N64"
echo "nginx 1m  $N1M"
nginx -e /tmp/atomos-bench-nginx.err -c "$TMP/nginx.conf" -s stop || true
kill_port 18092

echo "== h2o"
kill_port 18093
sed "s|ROOT|$TMP/www|g" "$BENCH/h2o.conf" >"$TMP/h2o.conf"
h2o -c "$TMP/h2o.conf" >/tmp/atomos-bench-h2o.log 2>&1 &
sleep 0.4
H11=$(wrk_json http://127.0.0.1:18093/11b)
H64=$(wrk_json http://127.0.0.1:18093/64k)
H1M=$(wrk_json http://127.0.0.1:18093/1m)
echo "h2o H1 11b $H11"
echo "h2o H1 64k $H64"
echo "h2o H1 1m  $H1M"
if command -v h2load >/dev/null 2>&1; then
  H2O_H2_MUX=$(h2load_json -p h2c -n50000 -c16 -m64 http://127.0.0.1:18093/11b)
  H2O_H2_SEQ=$(h2load_json -p h2c -n20000 -c1 -m1 http://127.0.0.1:18093/11b)
  echo "h2o h2c mux $H2O_H2_MUX"
  echo "h2o h2c seq $H2O_H2_SEQ"
else
  H2O_H2_MUX='{"rps":0,"succeeded":0,"failed":0,"mean_us":0,"max_us":0}'
  H2O_H2_SEQ='{"rps":0,"succeeded":0,"failed":0,"mean_us":0,"max_us":0}'
fi
killall h2o 2>/dev/null || true
kill_port 18093

python3 - <<'PY' "$TMP"
import subprocess, sys, pathlib
d = pathlib.Path(sys.argv[1])
subprocess.check_call(["openssl","req","-x509","-newkey","rsa:2048","-keyout",str(d/"key.pem"),"-out",str(d/"cert.pem"),"-days","1","-nodes","-subj","/CN=localhost"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
PY

echo "== atomos-proto TLS"
kill_port 18091
cat >"$TMP/atomos-proto-tls.json" <<EOF
{"bind":"127.0.0.1:18091","static_root":"$TMP/www","memory_cap_bytes":67108864,"engine":"tokio","workers":$WORKERS,"http2":true,"http3":true,"tls_cert":"$TMP/cert.pem","tls_key":"$TMP/key.pem"}
EOF
"$PROTO" --config "$TMP/atomos-proto-tls.json" --rules "$TMP/rules.json" >/tmp/atomos-bench-proto-tls.log 2>&1 &
sleep 0.8
P11=$(wrk_json https://127.0.0.1:18091/11b)
P64=$(wrk_json https://127.0.0.1:18091/64k)
echo "atomos proto TLS 11b $P11"
echo "atomos proto TLS 64k $P64"
if command -v h2load >/dev/null 2>&1; then
  P_H2_MUX=$(h2load_json -n50000 -c16 -m64 https://127.0.0.1:18091/11b)
  P_H2_SEQ=$(h2load_json -n20000 -c1 -m1 https://127.0.0.1:18091/11b)
  echo "atomos proto h2 tls mux $P_H2_MUX"
  echo "atomos proto h2 tls seq $P_H2_SEQ"
else
  P_H2_MUX='{"rps":0,"succeeded":0,"failed":0,"mean_us":0,"max_us":0}'
  P_H2_SEQ='{"rps":0,"succeeded":0,"failed":0,"mean_us":0,"max_us":0}'
fi
H3_SEQ='{}'
H3_MUX='{}'
if [[ -x $H23 ]]; then
  set +e
  H23_TLS=$("$H23" --h2-port 18091 --h3-port 18091 --count 2000 2>&1)
  set -e
  echo "$H23_TLS"
  H3_SEQ=$(python3 - <<'PY' "$H23_TLS"
import json, re, sys
text = sys.argv[1]
out = {"rps": 0.0, "p50_us": 0.0, "p99_us": 0.0}
# H3 sequential line after "## H3"
h3 = text.split("## H3")[-1] if "## H3" in text else ""
m = re.search(r"seq:\s*([\d.]+)\s*req/s:.*?p50\s*([\d.]+)us.*?p99\s*([\d.]+)us", h3, re.S)
if m:
    out = {"rps": float(m.group(1)), "p50_us": float(m.group(2)), "p99_us": float(m.group(3))}
print(json.dumps(out, separators=(",", ":")))
PY
)
  H3_MUX=$(python3 - <<'PY' "$H23_TLS"
import json, re, sys
text = sys.argv[1]
out = {"rps": 0.0}
h3 = text.split("## H3")[-1] if "## H3" in text else ""
m = re.search(r"mux x64:\s*([\d.]+)\s*req/s", h3)
if m:
    out["rps"] = float(m.group(1))
print(json.dumps(out, separators=(",", ":")))
PY
)
fi
kill_port 18091

echo "== atomos-proto h2c"
kill_port 18094
cat >"$TMP/atomos-proto-h2c.json" <<EOF
{"bind":"127.0.0.1:18094","static_root":"$TMP/www","memory_cap_bytes":67108864,"engine":"tokio","workers":$WORKERS,"http2":true,"http3":false}
EOF
"$PROTO" --config "$TMP/atomos-proto-h2c.json" --rules "$TMP/rules.json" >/tmp/atomos-bench-proto-h2c.log 2>&1 &
sleep 0.8
if command -v h2load >/dev/null 2>&1; then
  A_H2C_MUX=$(h2load_json -p h2c -n50000 -c16 -m64 http://127.0.0.1:18094/11b)
  A_H2C_SEQ=$(h2load_json -p h2c -n20000 -c1 -m1 http://127.0.0.1:18094/11b)
  echo "atomos h2c mux $A_H2C_MUX"
  echo "atomos h2c seq $A_H2C_SEQ"
else
  A_H2C_MUX='{"rps":0,"succeeded":0,"failed":0,"mean_us":0,"max_us":0}'
  A_H2C_SEQ='{"rps":0,"succeeded":0,"failed":0,"mean_us":0,"max_us":0}'
fi
H2_SEQ='{}'
H2_MUX='{}'
if [[ -x $H23 ]]; then
  set +e
  H23_H2=$("$H23" --h2-port 18094 --h3-port 18094 --count 2000 2>&1)
  set -e
  echo "$H23_H2"
  H2_SEQ=$(python3 - <<'PY' "$H23_H2"
import json, re, sys
text = sys.argv[1]
out = {"rps": 0.0, "p50_us": 0.0, "p90_us": 0.0, "p99_us": 0.0}
h2 = text.split("## H2")[-1].split("## H3")[0] if "## H2" in text else text
m = re.search(r"seq:\s*([\d.]+)\s*req/s:.*?p50\s*([\d.]+)us.*?p90\s*([\d.]+)us.*?p99\s*([\d.]+)us", h2, re.S)
if m:
    out = {"rps": float(m.group(1)), "p50_us": float(m.group(2)), "p90_us": float(m.group(3)), "p99_us": float(m.group(4))}
print(json.dumps(out, separators=(",", ":")))
PY
)
  H2_MUX=$(python3 - <<'PY' "$H23_H2"
import json, re, sys
text = sys.argv[1]
out = {"rps": 0.0}
h2 = text.split("## H2")[-1].split("## H3")[0] if "## H2" in text else text
m = re.search(r"mux x64:\s*([\d.]+)\s*req/s", h2)
if m:
    out["rps"] = float(m.group(1))
print(json.dumps(out, separators=(",", ":")))
PY
)
fi
kill_port 18094

python3 - "$OUT/$DATE.json" "$BENCH/baseline.json" <<PY
import json, sys, pathlib
outp, basep = sys.argv[1], sys.argv[2]
def j(s):
    try:
        return json.loads(s)
    except Exception:
        return {"rps": 0.0}
doc = {
    "method": {
        "load": "wrk -t4 -c256 -d15s --latency",
        "h2": "h2load -n50000 -c16 -m64 / -n20000 -c1 -m1",
        "h23": "bench_h23 --count 2000",
        "workers": $WORKERS,
    },
    "atomos_h1_plaintext": {"11b": j("""$A11"""), "64k": j("""$A64"""), "1m": j("""$A1M""")},
    "nginx": {"11b": j("""$N11"""), "64k": j("""$N64"""), "1m": j("""$N1M""")},
    "h2o": {
        "11b": j("""$H11"""), "64k": j("""$H64"""), "1m": j("""$H1M"""),
        "h2c_mux": j("""$H2O_H2_MUX"""), "h2c_seq": j("""$H2O_H2_SEQ"""),
    },
    "atomos_proto_tls": {"11b": j("""$P11"""), "64k": j("""$P64"""), "h2_mux": j("""$P_H2_MUX"""), "h2_seq": j("""$P_H2_SEQ""")},
    "atomos_h2c": {"h2load_mux": j("""$A_H2C_MUX"""), "h2load_seq": j("""$A_H2C_SEQ"""), "bench_seq": j("""$H2_SEQ"""), "bench_mux": j("""$H2_MUX""")},
    "atomos_h3": {"bench_seq": j("""$H3_SEQ"""), "bench_mux": j("""$H3_MUX""")},
}
pathlib.Path(outp).write_text(json.dumps(doc, indent=2) + "\n")
print(json.dumps(doc, indent=2))
base = json.loads(pathlib.Path(basep).read_text())
median = float(base["atomos_h1_plaintext_11b_rps"])
ratio = base.get("max_drop_ratio", 0.15)
got = float(doc["atomos_h1_plaintext"]["11b"].get("rps") or 0)
if got < median * (1.0 - ratio):
    raise SystemExit(f"H1 plaintext 11B {got} dropped more than {ratio*100:.0f}% vs median {median}")
PY
echo "wrote $OUT/$DATE.json"
