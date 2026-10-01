#!/usr/bin/env bash
# Run strict HTTP/2 conformance checks against a temporary Atomos server.
# Usage: scripts/test-h2spec.sh /absolute/path/to/h2spec
set -euo pipefail

CODE_ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
H2SPEC=$(command -v "${1:-h2spec}")
H2SPEC=$(cd "$(dirname "$H2SPEC")" && pwd)/$(basename "$H2SPEC")
PORT=${H2SPEC_PORT:-8090}
[[ "$PORT" =~ ^[0-9]+$ ]] && (( PORT > 0 && PORT <= 65535 )) || {
  echo 'H2SPEC_PORT must be between 1 and 65535' >&2
  exit 1
}
TMP=$(mktemp -d)
PID=""
cleanup() {
  local status=$?
  if [[ -n "$PID" ]]; then
    kill "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  if [[ $status -ne 0 && -f "$TMP/server.log" ]]; then
    printf '\nAtomos server log:\n' >&2
    cat "$TMP/server.log" >&2
  fi
  rm -rf "$TMP"
  exit "$status"
}
trap cleanup EXIT

cd "$CODE_ROOT"
cargo build --locked --features proto --bin atomos-proto
printf 'ok' > "$TMP/index.html"
cat > "$TMP/config.json" <<EOF
{"bind":"127.0.0.1:$PORT","static_root":"$TMP","memory_cap_bytes":67108864,"engine":"tokio","workers":1,"http2":true,"http3":false}
EOF
cat > "$TMP/rules.json" <<'EOF'
{"rules":[{"id":"s","module":"static","methods":["GET","HEAD","POST"],"include":["/*"],"exclude":[]}]}
EOF
"${CARGO_TARGET_DIR:-target}/debug/atomos-proto" --config "$TMP/config.json" --rules "$TMP/rules.json" > "$TMP/server.log" 2>&1 &
PID=$!
ready=false
for ((i = 0; i < 100; i++)); do
  kill -0 "$PID" || { echo 'Atomos exited before becoming ready' >&2; exit 1; }
  if (echo > "/dev/tcp/127.0.0.1/$PORT") >/dev/null 2>&1; then
    ready=true
    break
  fi
  sleep 0.1
done
if [[ "$ready" != true ]]; then
  echo 'Timed out waiting for Atomos' >&2
  exit 1
fi
kill -0 "$PID"
"$H2SPEC" -h 127.0.0.1 -p "$PORT" --strict
