#!/usr/bin/env bash
# One-at-a-time wrk harness. Nightly self-hosted Linux only.
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

echo "== compile"
(cd "$ROOT" && ./compile.sh)

BIN="$CODE/target/release/atomos"
PROTO="$CODE/target/release/atomos-proto"
if [[ ! -x $BIN ]]; then
  (cd "$CODE" && cargo build --release --bin atomos)
  BIN="$CODE/target/release/atomos"
fi
if [[ ! -x $PROTO ]]; then
  (cd "$CODE" && cargo build --release --features proto --bin atomos-proto)
  PROTO="$CODE/target/release/atomos-proto"
fi

TMP=$(mktemp -d)
trap 'kill $(jobs -p) 2>/dev/null || true; nginx -e /tmp/atomos-bench-nginx.err -c "$TMP/nginx.conf" -s stop 2>/dev/null || true; killall h2o 2>/dev/null || true; rm -rf "$TMP"' EXIT
mkdir -p "$TMP/www"
cp -a "$PAY/." "$TMP/www/"
ln -sfn 11b "$TMP/www/index.html" 2>/dev/null || cp "$PAY/11b" "$TMP/www/index.html"

wrk_rps() {
  local url=$1
  wrk -t4 -c256 -d15s --latency "$url" | awk '/Requests\/sec/{print $2}'
}

kill_port() {
  local p=$1
  if command -v fuser >/dev/null 2>&1; then
    fuser -k "${p}/tcp" 2>/dev/null || true
  else
    local pids
    pids=$(ss -H -ltnp "sport = :$p" 2>/dev/null | grep -oE 'pid=[0-9]+' | cut -d= -f2 | sort -u || true)
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

# --- Atomos H1 plaintext ---
kill_port 18090
cat >"$TMP/atomos-h1.json" <<EOF
{"bind":"127.0.0.1:18090","static_root":"$TMP/www","memory_cap_bytes":67108864,"engine":"epoll","workers":$WORKERS,"http2":false,"http3":false,"allow_non_loopback":false}
EOF
cat >"$TMP/rules.json" <<'EOF'
{"rules":[{"id":"s","module":"static","methods":["GET","HEAD"],"include":["/*"],"exclude":[]}]}
EOF
"$BIN" --config "$TMP/atomos-h1.json" --rules "$TMP/rules.json" >/tmp/atomos-bench-h1.log 2>&1 &
sleep 0.5
A11=$(wrk_rps http://127.0.0.1:18090/11b)
A64=$(wrk_rps http://127.0.0.1:18090/64k)
A1M=$(wrk_rps http://127.0.0.1:18090/1m)
kill_port 18090

# --- Atomos proto TLS ---
kill_port 18091
python3 - <<'PY' "$TMP"
import subprocess, sys, pathlib
d = pathlib.Path(sys.argv[1])
subprocess.check_call(["openssl","req","-x509","-newkey","rsa:2048","-keyout",str(d/"key.pem"),"-out",str(d/"cert.pem"),"-days","1","-nodes","-subj","/CN=localhost"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
PY
cat >"$TMP/atomos-proto.json" <<EOF
{"bind":"127.0.0.1:18091","static_root":"$TMP/www","memory_cap_bytes":67108864,"engine":"tokio","workers":$WORKERS,"http2":true,"http3":false,"tls_cert":"$TMP/cert.pem","tls_key":"$TMP/key.pem"}
EOF
"$PROTO" --config "$TMP/atomos-proto.json" --rules "$TMP/rules.json" >/tmp/atomos-bench-proto.log 2>&1 &
sleep 0.8
# wrk TLS: --tls if supported; else openssl s_client is too slow. Record even if we lose.
if wrk --help 2>&1 | grep -q '\-H'; then
  P11=$(wrk -t4 -c256 -d15s https://127.0.0.1:18091/11b 2>/dev/null | awk '/Requests\/sec/{print $2}' || echo 0)
  P64=$(wrk -t4 -c256 -d15s https://127.0.0.1:18091/64k 2>/dev/null | awk '/Requests\/sec/{print $2}' || echo 0)
else
  P11=0
  P64=0
fi
kill_port 18091

# --- nginx ---
kill_port 18092
sed "s|ROOT|$TMP/www|g" "$BENCH/nginx.conf" >"$TMP/nginx.conf"
nginx -e /tmp/atomos-bench-nginx.err -c "$TMP/nginx.conf"
sleep 0.3
N11=$(wrk_rps http://127.0.0.1:18092/11b)
N64=$(wrk_rps http://127.0.0.1:18092/64k)
N1M=$(wrk_rps http://127.0.0.1:18092/1m)
nginx -e /tmp/atomos-bench-nginx.err -c "$TMP/nginx.conf" -s stop || true
kill_port 18092

# --- h2o ---
kill_port 18093
sed "s|ROOT|$TMP/www|g" "$BENCH/h2o.conf" >"$TMP/h2o.conf"
h2o -c "$TMP/h2o.conf" >/tmp/atomos-bench-h2o.log 2>&1 &
sleep 0.4
H11=$(wrk_rps http://127.0.0.1:18093/11b)
H64=$(wrk_rps http://127.0.0.1:18093/64k)
H1M=$(wrk_rps http://127.0.0.1:18093/1m)
killall h2o 2>/dev/null || true
kill_port 18093

python3 - "$OUT/$DATE.json" "$A11" "$A64" "$A1M" "$P11" "$P64" "$N11" "$N64" "$N1M" "$H11" "$H64" "$H1M" "$BENCH/baseline.json" <<'PY'
import json, sys, pathlib
out, a11, a64, a1m, p11, p64, n11, n64, n1m, h11, h64, h1m, basep = sys.argv[1:]
def f(x):
    try:
        return float(x)
    except Exception:
        return 0.0
doc = {
    "atomos_h1_plaintext": {"11b": f(a11), "64k": f(a64), "1m": f(a1m)},
    "atomos_proto_tls": {"11b": f(p11), "64k": f(p64)},
    "nginx": {"11b": f(n11), "64k": f(n64), "1m": f(n1m)},
    "h2o": {"11b": f(h11), "64k": f(h64), "1m": f(h1m)},
}
pathlib.Path(out).write_text(json.dumps(doc, indent=2) + "\n")
print(json.dumps(doc, indent=2))
base = json.loads(pathlib.Path(basep).read_text())
median = float(base["atomos_h1_plaintext_11b_rps"])
ratio = base.get("max_drop_ratio", 0.15)
got = f(a11)
if got < median * (1.0 - ratio):
    raise SystemExit(f"H1 plaintext 11B {got} dropped more than {ratio*100:.0f}% vs median {median}")
PY
echo "wrote $OUT/$DATE.json"
