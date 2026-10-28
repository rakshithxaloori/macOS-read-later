#!/bin/sh
# Reproduce the required synthetic checks; personal migration and UI evidence
# are deliberately separate from these checks.
set -eu
umask 077
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"
RUN=$(date -u +%Y%m%dT%H%M%SZ)
OUT="$ROOT/.local/verification/$RUN"
mkdir -p "$OUT"
check() {
  label=$1
  shift
  printf 'Running %s\n' "$label"
  if "$ROOT/scripts/cargo-local.sh" "$@" > "$OUT/$label.log" 2>&1; then
    printf '%s passed\n' "$label"
  else
    cat "$OUT/$label.log"
    exit 1
  fi
}
check formatting fmt --all --check
check clippy clippy --locked --all-targets -- -D warnings
check tests test --locked --all-targets
check release build --locked --release
printf 'Verified source checks. Logs: %s\n' "$OUT"
