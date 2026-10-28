#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
DESTINATION="$ROOT/.local/dist/Read Later.app"
APP="$ROOT/.local/dist/.Read Later.build.app"
PREVIOUS="$ROOT/.local/dist/.Read Later.previous.app"
PROFILE=release
BUILD=yes
STAGE=no
for option in "$@"; do
  case "$option" in
    --no-build) BUILD=no ;;
    --debug) PROFILE=debug ;;
    --stage) STAGE=yes ;;
    *) printf 'Unknown bundle option: %s\n' "$option" >&2; exit 1 ;;
  esac
done
if [ "$STAGE" = yes ]; then
  DESTINATION="$ROOT/.local/dist/Read Later.lean.app"
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
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>Read Later</string>
<key>CFBundleDisplayName</key><string>Read Later</string>
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
# Inert path configuration makes this local handoff open the verified library.
# Explicit --data always overrides it; no source path is ever writable runtime data.
printf '%s\n' "$ROOT/.local/native" > "$APP/Contents/Resources/default-data-directory"
/usr/bin/plutil -lint "$APP/Contents/Info.plist"
/usr/bin/codesign --force --sign - "$APP"
/usr/bin/codesign --verify --strict "$APP"
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
