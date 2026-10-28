#!/bin/sh
set -eu
repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
# Prefer the isolated toolchain in .local/toolchain; fall back to the cargo on
# PATH so a fresh clone builds with an ordinary rustup install.
if [ -x "$repo/.local/toolchain/cargo/bin/cargo" ]; then
  export CARGO_HOME="$repo/.local/toolchain/cargo"
  export RUSTUP_HOME="$repo/.local/toolchain/rustup"
  export PATH="$CARGO_HOME/bin:$PATH"
fi
exec cargo "$@"
