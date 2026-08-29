#!/usr/bin/env bash
# Compile Atomos on this Linux host.
# Reads /proc and the compiler. Writes device files. Then runs cargo.
#
# Usage (from the repository root):
#   ./compile.sh              write flags and cargo build --release
#   ./compile.sh test         write flags and cargo test
#   ./compile.sh write        write Code/.cargo/config.toml and host.json
#   ./compile.sh print        print rustc flags; do not write
#
# Usage (from Code/):
#   ./scripts/compile.sh
#
# Git ignores the generated files. Do not copy them to another machine.

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
CODE_ROOT=$(cd "$SCRIPT_DIR/.." && pwd)

die() {
  echo "compile.sh: $*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing command: $1"
}

ver_ge() {
  # true if $1 >= $2 (sort -V).
  local first
  first=$(printf '%s\n' "$1" "$2" | sed 's/-.*//' | sort -V | head -1)
  [[ "$first" == "$(printf '%s' "$2" | sed 's/-.*//')" ]]
}

check_linux() {
  [[ "$(uname -s)" == Linux ]] || die "Linux only"
}

check_rust() {
  need_cmd rustc
  need_cmd cargo
  local have need="1.97.1"
  have=$(rustc --version | awk '{print $2}')
  ver_ge "$have" "$need" || die "rustc $need or later required (found $have)"
}

check_cc() {
  local c
  for c in "${ATOMOS_CC:-}" cc gcc clang; do
    [[ -z "$c" ]] && continue
    if command -v "$c" >/dev/null 2>&1; then
      return 0
    fi
  done
  die "need a C compiler (cc, gcc, or clang)"
}

write_device_files() {
  unset RUSTFLAGS || true
  # atomos-host.sh write also runs cpu-rustflags.sh write.
  "$SCRIPT_DIR/atomos-host.sh" write "$CODE_ROOT"
}

run_cargo() {
  unset RUSTFLAGS || true
  cd "$CODE_ROOT"
  cargo "$@"
}

cmd="${1:-release}"
case "$cmd" in
  release|build|"")
    check_linux
    check_rust
    check_cc
    write_device_files
    run_cargo build --release
    ;;
  test)
    check_linux
    check_rust
    check_cc
    write_device_files
    run_cargo test
    ;;
  write)
    check_linux
    check_cc
    write_device_files
    ;;
  print)
    check_linux
    "$SCRIPT_DIR/cpu-rustflags.sh" print
    "$SCRIPT_DIR/atomos-host.sh" print
    ;;
  -h|--help|help)
    sed -n '2,16p' "$0"
    ;;
  *)
    die "unknown command $cmd (try: release, test, write, print, help)"
    ;;
esac
