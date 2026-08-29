#!/usr/bin/env bash
# Device facts for Atomos. Reads /proc only. Never names a microarch.
#
# Prefer scripts/compile.sh. That script writes rustflags and this overlay.
#
#   scripts/atomos-host.sh              # print host.json
#   scripts/atomos-host.sh write [DIR]  # rustflags + DIR/.atomos/host.json
#   scripts/atomos-host.sh cargo …      # cpu-rustflags + cargo
#
# Optional: ATOMOS_REFUSE_PORTS=8082,80  (comma-separated)
# Optional: ATOMOS_HOST=/path/to/host.json  (runtime overlay)
# Git ignores .atomos/. Do not copy host.json to another machine.

set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

nproc_n() {
  if command -v nproc >/dev/null 2>&1; then
    nproc
    return
  fi
  grep -c '^processor' /proc/cpuinfo 2>/dev/null || echo 1
}

l3_bytes() {
  local kib=0
  if [[ -r /sys/devices/system/cpu/cpu0/cache/index3/size ]]; then
    local s
    s=$(tr -d '\n' < /sys/devices/system/cpu/cpu0/cache/index3/size)
    case "$s" in
      *K|*k) kib=${s%[Kk]} ;;
      *M|*m) kib=$(( ${s%[Mm]} * 1024 )) ;;
      *) kib=$s ;;
    esac
  elif [[ -r /proc/cpuinfo ]]; then
    kib=$(awk '/^cache size/{print $4; exit}' /proc/cpuinfo)
  fi
  if [[ -z "$kib" || "$kib" == "0" ]]; then
    echo 16777216
    return
  fi
  echo $((kib * 1024))
}

refuse_json_array() {
  local raw="${ATOMOS_REFUSE_PORTS:-}"
  if [[ -z "$raw" ]]; then
    echo "[]"
    return
  fi
  local out="[" first=1 p
  IFS=',' read -ra ps <<< "$raw"
  for p in "${ps[@]}"; do
    p=${p// /}
    [[ -z "$p" ]] && continue
    if [[ $first -eq 1 ]]; then
      first=0
    else
      out+=", "
    fi
    out+="$p"
  done
  out+="]"
  echo "$out"
}

host_json() {
  local n l3
  n=$(nproc_n)
  l3=$(l3_bytes)
  cat <<EOF
{
  "nproc": $n,
  "workers": $n,
  "cpu_pin": true,
  "cache_bytes": $l3,
  "cache_entries": 4096,
  "refuse_ports": $(refuse_json_array)
}
EOF
}

write_all() {
  local dir="${1:-$ROOT}"
  "$ROOT/scripts/cpu-rustflags.sh" write "$dir"
  mkdir -p "$dir/.atomos"
  host_json > "$dir/.atomos/host.json"
  echo "wrote $dir/.atomos/host.json"
}

cmd="${1:-print}"
case "$cmd" in
  print|"")
    host_json
    ;;
  write)
    write_all "${2:-}"
    ;;
  cargo)
    shift
    exec "$ROOT/scripts/cpu-rustflags.sh" cargo "$@"
    ;;
  -h|--help|help)
    sed -n '2,12p' "$0"
    ;;
  *)
    echo "atomos-host: unknown command $cmd" >&2
    exit 2
    ;;
esac
