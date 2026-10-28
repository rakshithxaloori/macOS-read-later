#!/bin/sh
# Move only reviewed Cargo outputs to Trash; preserve app and personal data.
set -eu
umask 077
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
TARGETS='target .local/targets/rendering .local/targets/migration .local/targets/whatsapp .local/targets/lean-storage .local/targets/lean-intake .local/targets/lean-magazines'
fail() { printf 'Refusing cleanup: %s\n' "$*" >&2; exit 1; }
case "${1:-}" in ''|--apply) ;; *) fail 'Usage: scripts/trash-builds.sh [--apply]' ;; esac
[ "$#" -le 1 ] || fail 'Too many arguments'
check() {
  for parent in .local .local/targets; do
    [ ! -L "$ROOT/$parent" ] || fail "$parent is a symlink"
  done
  path="$ROOT/$1"
  [ ! -L "$path" ] || fail "$1 is a symlink"
  [ -e "$path" ] || return 0
  [ -d "$path" ] || fail "$1 is not a directory"
  [ -f "$path/CACHEDIR.TAG" ] && [ ! -L "$path/CACHEDIR.TAG" ] || fail "$1 lacks a regular Cargo cache tag"
  IFS= read -r signature < "$path/CACHEDIR.TAG" || fail 'Cannot read cache tag'
  [ "$signature" = 'Signature: 8a477f597d28d172789f06886806bc55' ] || fail 'Unexpected cache tag'
  for entry in "$path"/* "$path"/.[!.]* "$path"/..?*; do
    [ -e "$entry" ] || [ -L "$entry" ] || continue
    case "${entry##*/}" in
      debug|release|tmp|CACHEDIR.TAG|.rustc_info.json) ;;
      *) fail "$1 contains an unexpected entry: ${entry##*/}" ;;
    esac
  done
}
check_processes() {
  processes=$(ps -axo pid=,comm=) || fail 'Cannot inspect processes'
  active=$(printf '%s\n' "$processes" | awk '{name=$0; sub(/^[[:space:]]*[0-9]+[[:space:]]+/,"",name); sub(/^.*\//,"",name); sub(/[[:space:]]+$/,"",name)} name=="cargo" || name=="rustc" {print $1,name}')
  [ -z "$active" ] || fail "Cargo/rustc is running: $active"
}
total=0
for relative in $TARGETS; do
  check "$relative"
  if [ -d "$ROOT/$relative" ]; then
    size=$(du -sk "$ROOT/$relative" | awk '{print $1}')
    total=$((total + size))
    printf '%12s KiB  %s\n' "$size" "$relative"
  fi
done
printf '%12s KiB  total\n' "$total"
[ "${1:-}" = --apply ] || { printf 'Preview only; use --apply to move these folders to Trash.\n'; exit 0; }
[ "$total" -gt 0 ] || exit 0
check_processes
TRASH="$HOME/.Trash"
[ -d "$TRASH" ] && [ ! -L "$TRASH" ] || fail 'Trash must be a regular directory'
mkdir -p "$ROOT/.local"
lock="$ROOT/.local/.storage-clean.lock"
mkdir "$lock" 2>/dev/null || fail 'Another cleanup is running or its lock remains'
trap 'rmdir "$lock"' EXIT
trap 'exit 1' HUP INT TERM
destination=$(mktemp -d "$TRASH/Read Later build cleanup.XXXXXXXX")
receipt="$ROOT/.local/trash-builds-$(date -u +%Y%m%dT%H%M%SZ).txt"
printf 'Trash destination: %s\nAllocated KiB: %s\n' "$destination" "$total" > "$receipt"
for relative in $TARGETS; do
  check "$relative"
  check_processes
  if [ -d "$ROOT/$relative" ]; then
    mkdir -p "$(dirname -- "$destination/$relative")"
    mv -- "$ROOT/$relative" "$destination/$relative"
    [ ! -e "$ROOT/$relative" ] && [ -f "$destination/$relative/CACHEDIR.TAG" ] || fail 'Move verification failed'
    printf 'Moved %s\n' "$relative" | tee -a "$receipt"
  fi
done
printf 'Trash destination: %s\nReceipt: %s\n' "$destination" "$receipt"
