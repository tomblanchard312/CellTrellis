#!/usr/bin/env bash
# Regenerate every app icon from assets/app-icon/nanshe.svg.
#
# Needs: resvg (brew install resvg / cargo install resvg). On macOS, iconutil also writes the
# .icns; elsewhere Python 3 writes the same PNG-backed container. The outputs are committed, so builds and packaging never need these tools.
#
#   packaging/icons.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="$ROOT/assets/app-icon"
SVG="$DIR/nanshe.svg"
ID="io.github.tomblanchard312.nanshe"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

command -v resvg >/dev/null || { echo "error: resvg not found (brew install resvg)" >&2; exit 1; }

# The artwork is a full-bleed 512 tile (rx=112). Windows and Linux use it as is. macOS icons sit on
# Apple's grid: an 824/1024 body with a transparent margin, so widen the viewBox to 512*1024/824.
MAC="$TMP/mac.svg"
sed 's/viewBox="0 0 512 512"/viewBox="-62.136 -62.136 636.272 636.272"/' "$SVG" >"$MAC"
grep -q 'viewBox="-62.136' "$MAC" || { echo "error: expected viewBox=\"0 0 512 512\" in $SVG" >&2; exit 1; }

render() {
  local svg="$1" size="$2" out="$3"
  resvg -w "$size" -h "$size" "$svg" "$out" </dev/null
}

render "$SVG" 1024 "$DIR/nanshe-1024.png"
# Runtime window/Dock icon on macOS (for apps/nanshe to embed); other platforms use hicolor 256.
render "$MAC" 512 "$DIR/nanshe-macos-512.png"

# Linux hicolor theme.
for s in 16 24 32 48 64 128 256 512; do
  mkdir -p "$DIR/hicolor/${s}x${s}/apps"
  render "$SVG" "$s" "$DIR/hicolor/${s}x${s}/apps/$ID.png"
done
mkdir -p "$DIR/hicolor/scalable/apps"
cp "$SVG" "$DIR/hicolor/scalable/apps/$ID.svg"

# Windows .ico.
ICO_PNGS=()
for s in 16 20 24 32 40 48 64 128 256; do
  render "$SVG" "$s" "$TMP/ico-$s.png"
  ICO_PNGS+=("$TMP/ico-$s.png")
done
(cd "$ROOT" && cargo run -q -p xtask --no-default-features -- ico "$DIR/nanshe.ico" "${ICO_PNGS[@]}")

# macOS .icns.
if command -v iconutil >/dev/null; then
  SET="$TMP/nanshe.iconset"
  mkdir -p "$SET"
  for s in 16 32 128 256 512; do
    render "$MAC" "$s" "$SET/icon_${s}x${s}.png"
    render "$MAC" $((s * 2)) "$SET/icon_${s}x${s}@2x.png"
  done
  iconutil -c icns -o "$DIR/nanshe.icns" "$SET"
else
  # PNG-backed ICNS entries also work without Apple's iconutil.
  for s in 16 32 64 128 256 512 1024; do
    render "$MAC" "$s" "$TMP/mac-$s.png"
  done
  python3 - "$DIR/nanshe.icns" "$TMP" <<'ICNS'
from pathlib import Path
import struct, sys
out, root = Path(sys.argv[1]), Path(sys.argv[2])
entries = [(b'icp4',16),(b'icp5',32),(b'icp6',64),(b'ic07',128),(b'ic08',256),
           (b'ic09',512),(b'ic10',1024),(b'ic11',32),(b'ic12',64),(b'ic13',256),(b'ic14',512)]
chunks = []
for tag, size in entries:
    data = (root / f'mac-{size}.png').read_bytes()
    chunks.append(tag + struct.pack('>I',len(data)+8) + data)
body = b''.join(chunks)
out.write_bytes(b'icns' + struct.pack('>I',len(body)+8) + body)
ICNS
fi
echo "icons written to $DIR"
