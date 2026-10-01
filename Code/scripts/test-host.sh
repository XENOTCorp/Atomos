#!/usr/bin/env bash
# Regression tests for host discovery, without modifying the working tree.
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin" "$TMP/project"
printf '[package]\nname = "host-test"\nversion = "0.1.0"\n' > "$TMP/project/Cargo.toml"

# A verbose compiler output exceeds a pipe buffer. Early-exit consumers used
# to SIGPIPE rustc and abort host builds when pipefail was enabled.
cat > "$TMP/bin/rustc" <<'EOF'
#!/usr/bin/env bash
if [[ "${1:-}" == -vV ]]; then
  printf 'host: x86_64-unknown-linux-gnu\n'
  for ((i = 0; i < 10000; i++)); do
    printf 'compiler metadata line %s\n' "$i"
  done
else
  printf 'rustc 1.97.1 (test)\n'
fi
EOF
chmod +x "$TMP/bin/rustc"
PATH="$TMP/bin:$PATH" "$SCRIPT_DIR/atomos-host.sh" write "$TMP/project"
grep -q '^\[target.x86_64-unknown-linux-gnu\]' "$TMP/project/.cargo/config.toml"
grep -q 'target-cpu=native' "$TMP/project/.cargo/config.toml"
grep -q '"workers":' "$TMP/project/.atomos/host.json"

# Also exercise the real compiler and CPU discovery on this host.
"$SCRIPT_DIR/cpu-rustflags.sh" print > "$TMP/flags"
"$SCRIPT_DIR/atomos-host.sh" print > "$TMP/host.json"
grep -q 'target-cpu=native' "$TMP/flags"
# Generated flags must work on stable Rust without unstable-feature warnings.
read -ra flags < "$TMP/flags"
printf 'fn main() {}\n' > "$TMP/probe.rs"
rustc "${flags[@]}" --deny warnings --emit metadata --crate-name host_probe \
  "$TMP/probe.rs" -o "$TMP/probe.rmeta"
printf 'host discovery tests passed\n'
