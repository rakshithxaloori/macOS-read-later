#!/bin/sh
# Cut a versioned release from a clean main: verify, tag, bundle and archive.
# Installing into /Applications stays a separate, explicitly approved step.
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"
fail() { printf 'Refusing release: %s\n' "$*" >&2; exit 1; }
VERSION=${1:-}
case "$VERSION" in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) fail 'Usage: scripts/release.sh X.Y.Z' ;;
esac
CARGO_VERSION=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
[ "$CARGO_VERSION" = "$VERSION" ] || fail "Cargo.toml says $CARGO_VERSION; bump it to $VERSION first"
grep -q "^## \[$VERSION\]" CHANGELOG.md || fail "CHANGELOG.md has no ## [$VERSION] section"
[ "$(git branch --show-current)" = main ] || fail 'Releases are cut from main'
[ -z "$(git status --porcelain)" ] || fail 'Working tree has uncommitted changes'
! git rev-parse -q --verify "refs/tags/v$VERSION" >/dev/null || fail "Tag v$VERSION already exists"
CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2} "$ROOT/scripts/verify.sh"
git tag -a "v$VERSION" -m "Read Later $VERSION"
"$ROOT/packaging/macos/bundle.sh" --no-build
OUT="$ROOT/.local/releases"
mkdir -p "$OUT"
ARCHIVE="$OUT/Read-Later-$VERSION.zip"
rm -f "$ARCHIVE"
/usr/bin/ditto -c -k --keepParent "$ROOT/.local/dist/Read Later.app" "$ARCHIVE"
shasum -a 256 "$ARCHIVE" | tee "$ARCHIVE.sha256"
printf 'Release v%s is tagged and archived. Install only after approval.\n' "$VERSION"
