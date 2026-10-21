#!/usr/bin/env bash
# Builds packaging/AppIcon.icns and docs/logo-256.png from docs/logo.svg (macOS: swiftc, iconutil).
set -euo pipefail
cd "$(dirname "$0")/.."
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
swiftc -O scripts/icon/svg2png.swift -o "$tmp/svg2png"
mkdir -p "$tmp/AppIcon.iconset"
for s in 16 32 128 256 512; do
  # "pad": the 10 % margin of macOS app icons.
  "$tmp/svg2png" docs/logo.svg "$tmp/AppIcon.iconset/icon_${s}x${s}.png" "$s" pad
  "$tmp/svg2png" docs/logo.svg "$tmp/AppIcon.iconset/icon_${s}x${s}@2x.png" "$((s * 2))" pad
done
iconutil -c icns "$tmp/AppIcon.iconset" -o packaging/AppIcon.icns
"$tmp/svg2png" docs/logo.svg docs/logo-256.png 256
echo "packaging/AppIcon.icns, docs/logo-256.png"
