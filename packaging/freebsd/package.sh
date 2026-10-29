#!/usr/bin/env bash
# Build and package Nanshe for FreeBSD (<arch> is x86_64 or aarch64):
#
#   $DIST/nanshe-<version>-freebsd-<arch>.tar.gz   bin/ + share/ (desktop entry, icons,
#                                                      AppStream, MIME), extract under /usr/local
#
# Usage: packaging/freebsd/package.sh [--skip-build]
#
# Runs on FreeBSD itself (CI: .github/workflows/freebsd.yml, in a FreeBSD VM). Needs cargo and
# the build deps listed in that workflow. Runtime deps on the user's system: libxkbcommon,
# wayland or libX11/libXcursor/libXrandr/libXi, mesa-libs or vulkan-loader.
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
APP_ID=io.github.tomblanchard312.nanshe
LINUX="$ROOT/packaging/linux"

SKIP_BUILD=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    -h | --help) sed -n '2,11p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

# Cross-builds (release workflow sets CARGO_BUILD_TARGET) take the arch from the target, not the host.
if [[ -n "${CARGO_BUILD_TARGET:-}" ]]; then
  case "$CARGO_BUILD_TARGET" in
    aarch64-unknown-freebsd) ARCH=aarch64 ;;
    x86_64-unknown-freebsd) ARCH=x86_64 ;;
    *) echo "unsupported CARGO_BUILD_TARGET $CARGO_BUILD_TARGET (expected x86_64-unknown-freebsd or aarch64-unknown-freebsd)" >&2; exit 2 ;;
  esac
else
  ARCH="$(uname -m)"
  case "$ARCH" in
    amd64 | x86_64) ARCH=x86_64 ;;
    arm64 | aarch64) ARCH=aarch64 ;;
    *) echo "unsupported architecture $ARCH" >&2; exit 2 ;;
  esac
fi
BASENAME="nanshe-$VERSION-freebsd-$ARCH"

echo "==> Nanshe $VERSION for FreeBSD $ARCH"

if [[ "$SKIP_BUILD" = 0 ]]; then
  (cd "$ROOT" && cargo build --release --locked -p nanshe -p nanshe-cli ${CARGO_BUILD_TARGET:+--target "$CARGO_BUILD_TARGET"})
fi
BIN="$CARGO_TARGET_DIR/${CARGO_BUILD_TARGET:+$CARGO_BUILD_TARGET/}release"
WORK="$CARGO_TARGET_DIR/freebsd-package"
TREE="$WORK/$BASENAME"
rm -rf "$WORK"

install -d "$TREE/bin" "$TREE/share/applications" "$TREE/share/metainfo" "$TREE/share/mime/packages" \
  "$TREE/share/icons" "$TREE/share/doc/nanshe"
install -m 755 "$BIN/nanshe" "$BIN/nanshe-cli" "$TREE/bin/"
strip "$TREE/bin/nanshe" "$TREE/bin/nanshe-cli" 2>/dev/null || true
install -m 644 "$LINUX/$APP_ID.desktop" "$TREE/share/applications/$APP_ID.desktop"
install -m 644 "$LINUX/$APP_ID.mime.xml" "$TREE/share/mime/packages/$APP_ID.xml"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@DATE@/$NANSHE_BUILD_DATE/g" \
  "$LINUX/$APP_ID.metainfo.xml.in" >"$TREE/share/metainfo/$APP_ID.metainfo.xml"
cp -R "$ROOT/assets/app-icon/hicolor" "$TREE/share/icons/"
copy_docs "$TREE/share/doc/nanshe"

tar -C "$WORK" -czf "$DIST/$BASENAME.tar.gz" "$BASENAME"
echo "wrote $DIST/$BASENAME.tar.gz"

# A cross-built binary cannot run on the x86_64 host.
if [[ -z "${CARGO_BUILD_TARGET:-}" ]]; then "$TREE/bin/nanshe-cli" --version; fi
echo "==> done"
ls -lh "$DIST/$BASENAME.tar.gz"
