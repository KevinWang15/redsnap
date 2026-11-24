#!/usr/bin/env bash
# Assembles, signs, and zips RedSnap.app for macOS.
#
# Usage:
#   packaging/macos/package.sh <out-dir> <binary> [<second-arch-binary>]
#
# Pass one binary for a single-architecture bundle, or both arm64 and x86_64
# binaries to combine them into a universal binary via `lipo`.
# Set CODESIGN_IDENTITY to a Developer ID Application identity to sign with
# hardened runtime; otherwise the bundle is ad-hoc signed (`-`).

set -euo pipefail

if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
    echo "Usage: $0 <out-dir> <binary> [<second-arch-binary>]" >&2
    exit 2
fi

OUT_DIR="$1"
BIN1="$2"
BIN2="${3:-}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

VERSION="$(sed -nE 's/^version = "([^"]+)"/\1/p' "$REPO_ROOT/Cargo.toml" | head -n 1)"
if [ -z "$VERSION" ]; then
    echo "error: could not read workspace version from Cargo.toml" >&2
    exit 1
fi

APP_DIR="$OUT_DIR/RedSnap.app"
ZIP_PATH="$OUT_DIR/RedSnap-macos.zip"

rm -rf "$APP_DIR" "$ZIP_PATH"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"

if [ -n "$BIN2" ]; then
    lipo -create -output "$APP_DIR/Contents/MacOS/redsnap" "$BIN1" "$BIN2"
else
    cp "$BIN1" "$APP_DIR/Contents/MacOS/redsnap"
fi
chmod 755 "$APP_DIR/Contents/MacOS/redsnap"

cp "$REPO_ROOT/packaging/icons/redsnap.icns" "$APP_DIR/Contents/Resources/RedSnap.icns"
sed "s/__VERSION__/$VERSION/g" "$SCRIPT_DIR/Info.plist" > "$APP_DIR/Contents/Info.plist"
printf 'APPL????' > "$APP_DIR/Contents/PkgInfo"

plutil -lint "$APP_DIR/Contents/Info.plist"

IDENTITY="${CODESIGN_IDENTITY:--}"
if [ "$IDENTITY" = "-" ]; then
    codesign --force --deep --sign - "$APP_DIR"
else
    codesign --force --deep --options runtime --timestamp --sign "$IDENTITY" "$APP_DIR"
fi
codesign --verify --deep --strict --verbose=2 "$APP_DIR"

ditto -c -k --sequesterRsrc --keepParent "$APP_DIR" "$ZIP_PATH"
echo "Packaged $APP_DIR and $ZIP_PATH (version $VERSION)"
file "$APP_DIR/Contents/MacOS/redsnap"
