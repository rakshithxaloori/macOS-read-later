#!/bin/sh
# Build the downloadable disk image: a portable Reader.app (no link to this
# checkout's library) next to an Applications shortcut.
# Usage: scripts/dmg.sh [--no-build]
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
VERSION=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$ROOT/Cargo.toml" | head -n 1)
[ -n "$VERSION" ] || { printf 'Cannot read version from Cargo.toml\n' >&2; exit 1; }
"$ROOT/packaging/macos/bundle.sh" --portable "$@" >/dev/null
APP="$ROOT/.local/dist/portable/Reader.app"
[ ! -e "$APP/Contents/Resources/default-data-directory" ] || { printf 'Portable bundle must not carry a data directory\n' >&2; exit 1; }
OUT="$ROOT/.local/releases"
STAGING="$OUT/.dmg-staging"
DMG="$OUT/Reader-$VERSION.dmg"
mkdir -p "$OUT"
rm -rf "$STAGING" "$DMG"
mkdir -p "$STAGING"
/usr/bin/ditto "$APP" "$STAGING/Reader.app"
ln -s /Applications "$STAGING/Applications"
/usr/bin/hdiutil create -quiet -volname "Reader" -srcfolder "$STAGING" -fs HFS+ -format UDZO -ov "$DMG"
rm -rf "$STAGING"
# The website links to the stable name Reader.dmg on the latest release.
cp "$DMG" "$OUT/Reader.dmg"
shasum -a 256 "$DMG" | tee "$DMG.sha256"
printf '%s\n' "$DMG"
