#!/bin/sh
# Development storage maintenance. No application or personal data is removed.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
TARGETS='target .local/targets/rendering .local/targets/migration .local/targets/whatsapp'

help() {
  cat <<'EOF'
Usage: scripts/storage.sh [audit | clean-builds [--apply] | --help]

audit                    Report allocated storage in KiB; default action.
clean-builds             Preview the four enumerated Cargo target directories.
clean-builds --apply     Remove those build outputs after safety checks.

Preserves .local/native, historical archives, toolchain/dependency caches,
the packaged .app, screenshots, and verification evidence. Never traverses
symlinks. Stop Cargo/rustc before applying; do not start builds during cleanup.
Any running Cargo/rustc process blocks cleanup, including unrelated projects.
EOF
}

fail() { printf 'Refusing cleanup: %s\n' "$*" >&2; exit 1; }

check_parents() {
  for path in .local .local/targets; do
    [ ! -L "$ROOT/$path" ] || fail "$path is a symlink"
    if [ -e "$ROOT/$path" ]; then
      [ -d "$ROOT/$path" ] || fail "$path is not a directory"
    fi
  done
}

check_target() {
  relative=$1
  path="$ROOT/$relative"
  [ ! -L "$path" ] || fail "$relative is a symlink"
  [ -e "$path" ] || return 0
  [ -d "$path" ] || fail "$relative is not a directory"
  [ -f "$path/CACHEDIR.TAG" ] && [ ! -L "$path/CACHEDIR.TAG" ] ||
    fail "$relative lacks a regular Cargo cache tag"
  IFS= read -r signature < "$path/CACHEDIR.TAG" || fail "cannot read $relative cache tag"
  [ "$signature" = 'Signature: 8a477f597d28d172789f06886806bc55' ] ||
    fail "$relative has an unexpected cache tag"
  [ -f "$path/.rustc_info.json" ] && [ ! -L "$path/.rustc_info.json" ] ||
    fail "$relative lacks regular Cargo compiler metadata"
  # A new top-level name needs explicit review before it can be discarded.
  for entry in "$path"/* "$path"/.[!.]* "$path"/..?*; do
    [ -e "$entry" ] || [ -L "$entry" ] || continue
    case "${entry##*/}" in
      debug|release|tmp|CACHEDIR.TAG|.rustc_info.json) ;;
      *) fail "$relative contains an unexpected entry: ${entry##*/}" ;;
    esac
  done
}

check_processes() {
  # Inspect executable names, not arguments that may quote cargo/rustc.
  # Conservative scope avoids missing a Cargo command whose cwd is this repo.
  processes=$(ps -axo pid=,comm=) || fail "cannot inspect running processes"
  active=$(printf '%s\n' "$processes" | awk '
    {
      name = $0
      sub(/^[[:space:]]*[0-9]+[[:space:]]+/, "", name)
      sub(/^.*\//, "", name)
      sub(/[[:space:]]+$/, "", name)
    }
    name == "cargo" || name == "rustc" { print $1, name }
  ')
  [ -z "$active" ] || fail "Cargo/rustc is running (PID and executable):
$active"
}

preview() {
  check_parents
  total=0
  printf 'Enumerated Cargo build outputs (KiB):\n'
  for relative in $TARGETS; do
    check_target "$relative"
    if [ -d "$ROOT/$relative" ]; then
      size=$(du -sk "$ROOT/$relative" | awk '{ print $1 }')
      total=$((total + size))
      printf '%12s  %s\n' "$size" "$relative"
    else
      printf '%12s  %s (absent)\n' 0 "$relative"
    fi
  done
  printf '%12s  total reclaimable KiB\n' "$total"
}

command=${1:-audit}
case "$command" in
  --help|-h|help) [ "$#" -le 1 ] || { help >&2; exit 2; }; help ;;
  audit)
    [ "$#" -le 1 ] || { help >&2; exit 2; }
    printf 'Repository allocation (KiB), symlinks are not followed:\n'
    du -k -d 1 "$ROOT"
    if [ -d "$ROOT/.local" ] && [ ! -L "$ROOT/.local" ]; then
      printf '\nPrivate storage allocation (KiB):\n'
      du -k -d 1 "$ROOT/.local"
    fi
    printf '\n'
    preview
    ;;
  clean-builds)
    [ "$#" -le 2 ] || { help >&2; exit 2; }
    apply=${2:-}
    case "$apply" in ''|--apply) ;; *) help >&2; exit 2 ;; esac
    preview
    if [ "$apply" != --apply ]; then
      printf '\nPreview only. To remove these outputs: scripts/storage.sh clean-builds --apply\n'
      exit 0
    fi
    check_processes
    # Serialize maintenance commands. Build commands do not take this lock.
    mkdir -p "$ROOT/.local"
    lock="$ROOT/.local/.storage-clean.lock"
    mkdir "$lock" 2>/dev/null || fail "another cleanup is running, or $lock remains after interruption"
    trap 'rmdir "$lock"' EXIT
    trap 'exit 1' HUP INT TERM
    for relative in $TARGETS; do
      check_parents
      check_target "$relative"
      check_processes
      if [ -d "$ROOT/$relative" ]; then
        # rm unlinks nested symlinks; it does not follow their destinations.
        rm -rf -- "$ROOT/$relative"
        printf 'Removed %s\n' "$relative"
      fi
    done
    ;;
  *) help >&2; exit 2 ;;
esac
