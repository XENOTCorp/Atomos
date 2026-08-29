#!/usr/bin/env bash
# Derive cargo/rustc flags from this Linux host.
# No microarch name. No hardcoded triple. Linker is detected.
#
# Usage:
#   scripts/cpu-rustflags.sh              # print rustc -C args
#   scripts/cpu-rustflags.sh features     # print detected SIMD names
#   eval "$(scripts/cpu-rustflags.sh export)"
#   scripts/cpu-rustflags.sh write [REPO] # rewrite REPO/.cargo/config.toml
#   scripts/cpu-rustflags.sh cargo test
#
# Optional environment:
#   ATOMOS_CC   C compiler for rustc linker= (cc, gcc, clang)
#   ATOMOS_LD   lld, mold, or empty for the compiler default
#   EXTRA_RUSTFLAGS  appended by the export command
#
# Host RUSTFLAGS overrides .cargo/config.toml. Unset it before cargo.

set -euo pipefail

is_x86() {
  case "$(uname -m)" in
    x86_64|amd64|i386|i686) return 0 ;;
    *) return 1 ;;
  esac
}

cpuinfo_flags() {
  if [[ ! -r /proc/cpuinfo ]]; then
    echo "cpu-rustflags: cannot read /proc/cpuinfo" >&2
    return 1
  fi
  # x86 uses "flags:"; aarch64 uses "Features:".
  grep -m1 -E '^(flags|Features)[[:space:]]*:' /proc/cpuinfo \
    | sed 's/^[^:]*:[[:space:]]*//' \
    | tr '[:upper:]' '[:lower:]'
}

has_flag() {
  local needle=" $1 "
  local hay=" $2 "
  [[ "$hay" == *"$needle"* ]]
}

AVX512_RUSTC=(
  avx512f
  avx512cd
  avx512dq
  avx512bw
  avx512vl
  avx512ifma
  avx512vbmi
  avx512vbmi2
  avx512vnni
  avx512bitalg
  avx512vpopcntdq
  avx512bf16
  avx512vp2intersect
  avx512fp16
)

linux_has_avx512() {
  local flags="$1"
  has_flag avx512f "$flags" || has_flag avx512 "$flags"
}

linux_has_avx10() {
  local flags="$1"
  has_flag avx10.1 "$flags" || has_flag avx10_1 "$flags" || has_flag avx10 "$flags"
}

detect_cc() {
  if [[ -n "${ATOMOS_CC:-}" ]]; then
    echo "$ATOMOS_CC"
    return 0
  fi
  local c
  for c in cc gcc clang; do
    if command -v "$c" >/dev/null 2>&1; then
      echo "$c"
      return 0
    fi
  done
  echo "cpu-rustflags: need cc, gcc, or clang" >&2
  return 1
}

detect_ld() {
  if [[ -n "${ATOMOS_LD+x}" ]]; then
    echo "${ATOMOS_LD}"
    return 0
  fi
  if command -v ld.lld >/dev/null 2>&1 || command -v lld >/dev/null 2>&1; then
    echo lld
    return 0
  fi
  if command -v mold >/dev/null 2>&1; then
    echo mold
    return 0
  fi
  echo ""
}

rustc_host() {
  if ! command -v rustc >/dev/null 2>&1; then
    return 0
  fi
  rustc -vV 2>/dev/null | awk '/^host:/{print $2; exit}'
}

detect_avx() {
  if ! is_x86; then
    return 0
  fi
  local flags present=()
  flags=$(cpuinfo_flags)
  if has_flag avx "$flags"; then present+=(avx); fi
  if has_flag avx2 "$flags"; then present+=(avx2); fi
  if linux_has_avx512 "$flags"; then
    local f
    for f in "${AVX512_RUSTC[@]}"; do
      if has_flag "$f" "$flags"; then
        present+=("$f")
      fi
    done
    local joined=" ${present[*]} "
    if [[ "$joined" != *" avx512f "* ]]; then
      present+=(avx512f)
    fi
  fi
  if linux_has_avx10 "$flags"; then
    present+=("avx10.1")
  fi
  printf '%s\n' "${present[@]}"
}

target_feature_csv() {
  if ! is_x86; then
    echo ""
    return 0
  fi
  local flags present=() f
  flags=$(cpuinfo_flags)
  mapfile -t present < <(detect_avx)

  local parts=()
  for f in "${present[@]}"; do
    [[ -n "$f" ]] && parts+=("+$f")
  done

  if ! linux_has_avx512 "$flags"; then
    for f in "${AVX512_RUSTC[@]}"; do
      parts+=("-$f")
    done
  fi
  if ! linux_has_avx10 "$flags"; then
    parts+=("-avx10.1" "-avx10.2")
  fi

  local IFS=,
  echo "${parts[*]}"
}

# Fill nameref array with rustc -C tokens: flag then value.
rustc_c_pairs() {
  local -n _out=$1
  _out=()
  local feat ld
  feat=$(target_feature_csv)
  ld=$(detect_ld)
  _out+=("-C" "target-cpu=native")
  if [[ -n "$feat" ]]; then
    _out+=("-C" "target-feature=${feat}")
  fi
  if [[ -n "$ld" ]]; then
    _out+=("-C" "link-arg=-fuse-ld=${ld}")
  fi
  _out+=("-C" "link-arg=-Wl,-z,relro,-z,now,-z,noexecstack")
}

toml_rustflags_array() {
  local pairs=() i
  rustc_c_pairs pairs
  printf '[\n  '
  for i in "${!pairs[@]}"; do
    if [[ $i -gt 0 ]]; then
      printf ', '
    fi
    printf '"%s"' "${pairs[$i]}"
  done
  printf '\n]\n'
}

export_line() {
  local pairs=() joined="" a
  rustc_c_pairs pairs
  for a in "${pairs[@]}"; do
    if [[ -n "$joined" ]]; then
      joined+=" "
    fi
    joined+="$a"
  done
  printf "export RUSTFLAGS='%s%s'\n" "$joined" "${EXTRA_RUSTFLAGS:+ $EXTRA_RUSTFLAGS}"
}

write_cargo_config() {
  local root="${1:-}"
  if [[ -z "$root" ]]; then
    root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
  fi
  if [[ ! -f "$root/Cargo.toml" ]]; then
    echo "cpu-rustflags: no Cargo.toml under $root" >&2
    return 1
  fi
  local dest="$root/.cargo/config.toml"
  mkdir -p "$root/.cargo"
  local present host cc ld simd=()
  mapfile -t simd < <(detect_avx)
  present="${simd[*]}"
  if [[ -z "$present" ]]; then
    if is_x86; then
      present="(none)"
    else
      present="native ($(uname -m))"
    fi
  fi
  host=$(rustc_host)
  cc=$(detect_cc)
  ld=$(detect_ld)
  local arr
  arr=$(toml_rustflags_array)
  {
    echo "# Generated by scripts/cpu-rustflags.sh on $(date -u +%Y-%m-%dT%H:%MZ)."
    echo "# Host uname: $(uname -s) $(uname -m). SIMD: ${present}."
    echo "# linker=${cc} ld=${ld:-compiler-default} triple=${host:-unknown}."
    echo "# Git ignores this file. Run scripts/compile.sh on each machine."
    echo
    echo "[build]"
    echo "rustflags = ${arr}"
    if [[ -n "$host" ]]; then
      echo
      echo "[target.${host}]"
      echo "linker = \"${cc}\""
      echo "rustflags = ${arr}"
    fi
  } >"$dest"
  echo "wrote $dest"
}

cmd="${1:-print}"
case "$cmd" in
  print|"")
    pairs=()
    rustc_c_pairs pairs
    printf '%s ' "${pairs[@]}"
    echo
    ;;
  features)
    detect_avx
    ;;
  export)
    export_line
    ;;
  write)
    write_cargo_config "${2:-}"
    ;;
  cargo)
    shift
    eval "$(export_line)"
    exec cargo "$@"
    ;;
  -h|--help|help)
    sed -n '2,16p' "$0"
    ;;
  *)
    echo "cpu-rustflags: unknown command $cmd" >&2
    exit 2
    ;;
esac
