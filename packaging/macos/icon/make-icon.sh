#!/bin/sh
# Regenerate AppIcon.icns and the runtime PNG from draw-icon.swift.
set -eu
DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
swift "$DIR/draw-icon.swift" "$WORK/icon-1024.png"
SET="$WORK/AppIcon.iconset"
mkdir "$SET"
for s in 16 32 128 256 512; do
  sips -z $s $s "$WORK/icon-1024.png" --out "$SET/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z $d $d "$WORK/icon-1024.png" --out "$SET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$SET" -o "$DIR/AppIcon.icns"
sips -z 256 256 "$WORK/icon-1024.png" --out "$DIR/icon-256.png" >/dev/null
printf '%s\n' "$DIR/AppIcon.icns"
