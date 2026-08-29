#!/usr/bin/env bash
# Compile Atomos on this Linux host. Wrapper for Code/scripts/compile.sh.
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
exec "$ROOT/Code/scripts/compile.sh" "$@"
