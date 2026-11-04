#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
DESTINATION="$ROOT/.local/dist/Reader.app"
APP="$ROOT/.local/dist/.Reader.build.app"
PREVIOUS="$ROOT/.local/dist/.Reader.previous.app"
PROFILE=release
BUILD=yes
STAGE=no
PORTABLE=no
for option in "$@"; do
  case "$option" in
    --no-build) BUILD=no ;;
    --debug) PROFILE=debug ;;
    --stage) STAGE=yes ;;
    --portable) PORTABLE=yes ;;
    *) printf 'Unknown bundle option: %s\n' "$option" >&2; exit 1 ;;
  esac
done
if [ "$PORTABLE" = yes ]; then
  # A portable bundle is what other people download: it never points at this
  # checkout's library, so it opens its own Application Support library.
  mkdir -p "$ROOT/.local/dist/portable"
  DESTINATION="$ROOT/.local/dist/portable/Reader.app"
  APP="$ROOT/.local/dist/portable/.Reader.build.app"
  PREVIOUS="$ROOT/.local/dist/portable/.Reader.previous.app"
fi
if [ "$STAGE" = yes ]; then
  DESTINATION="$ROOT/.local/dist/Reader.lean.app"
  if [ -e "$DESTINATION" ]; then
    printf 'Staged bundle already exists: %s\n' "$DESTINATION" >&2
    exit 1
  fi
fi
if [ "$BUILD" = yes ]; then
  if [ "$PROFILE" = release ]; then
    "$ROOT/scripts/cargo-local.sh" build --locked --release
  else
    "$ROOT/scripts/cargo-local.sh" build --locked
  fi
fi
# Cargo.toml is the single source of the marketing version; the build number
# counts commits so every release from main gets a larger CFBundleVersion.
VERSION=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$ROOT/Cargo.toml" | head -n 1)
[ -n "$VERSION" ] || { printf 'Cannot read version from Cargo.toml\n' >&2; exit 1; }
BUILD_NUMBER=$(git -C "$ROOT" rev-list --count HEAD 2>/dev/null || printf '0')
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$ROOT/target/$PROFILE/macos-read-later" "$APP/Contents/MacOS/macos-read-later"
chmod 755 "$APP/Contents/MacOS/macos-read-later"
# The bundle icon is what Finder and the Dock show while the app is not running.
cp "$ROOT/packaging/macos/icon/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"
# The app is named Reader, but CFBundleIdentifier keeps its original value:
# existing installs, Keychain access and macOS permissions are tied to it.
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>Reader</string>
<key>CFBundleDisplayName</key><string>Reader</string>
<key>CFBundleIdentifier</key><string>org.reading-list.native</string>
<key>CFBundleExecutable</key><string>macos-read-later</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleIconFile</key><string>AppIcon</string>
<key>CFBundleShortVersionString</key><string>$VERSION</string>
<key>CFBundleVersion</key><string>$BUILD_NUMBER</string>
<key>NSHumanReadableCopyright</key><string>© 2026 Rakshith Aloori</string>
<key>LSMinimumSystemVersion</key><string>13.0</string>
<key>NSHighResolutionCapable</key><true/>
<key>NSPrincipalClass</key><string>NSApplication</string>
</dict></plist>
PLIST
# Inert path configuration makes this local handoff open the library kept in
# this checkout's .local/native, when one exists. A fresh clone or fork has none,
# so its app uses Application Support. Explicit --data always overrides it.
if [ "$PORTABLE" = no ] && [ -d "$ROOT/.local/native" ]; then
  printf '%s\n' "$ROOT/.local/native" > "$APP/Contents/Resources/default-data-directory"
fi
/usr/bin/plutil -lint "$APP/Contents/Info.plist"
/usr/bin/codesign --force --sign - "$APP"
/usr/bin/codesign --verify --strict "$APP"
# The app bundle is the only size budget; the library is unbounded.
LIMIT=50000000
BYTES=$(( $(du -sk "$APP" | cut -f1) * 1024 ))
if [ "$BYTES" -ge "$LIMIT" ]; then
  printf 'App bundle is %s bytes; it must stay below %s bytes (50 MB)\n' "$BYTES" "$LIMIT" >&2
  rm -rf "$APP"
  exit 1
fi
printf 'App bundle: %s bytes (limit %s)\n' "$BYTES" "$LIMIT" >&2
# Never rewrite an executable in a running bundle. Publish the complete signed
# directory by rename, preserving mapped executable bytes in the old process.
if [ "$STAGE" = no ]; then
  rm -rf "$PREVIOUS"
  if [ -d "$DESTINATION" ]; then
    mv "$DESTINATION" "$PREVIOUS"
  fi
fi
mv "$APP" "$DESTINATION"
printf '%s\n' "$DESTINATION"
