#!/usr/bin/env bash
# Builds dist/Live Notes.app (release) and signs it.
#
#   scripts/bundle-macos.sh                 ad-hoc signature (local use)
#   LN_SIGN_IDENTITY="Developer ID Application: …" scripts/bundle-macos.sh
#
# A stable identity keeps macOS from asking for microphone/audio permission after every
# build; ad-hoc signatures change with each build (SPEC §16).
# llama-server: put a self-contained build at $LN_LLAMA_SERVER to embed it; otherwise the
# app uses llama-server from PATH / Homebrew (`brew install llama.cpp`).
set -euo pipefail
cd "$(dirname "$0")/.."
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
APP="dist/Live Notes.app"
# LN_PROFILE=dev builds a quick debug bundle for the development loop.
PROFILE="${LN_PROFILE:-release}"
# LN_FEATURES=screenshots adds the verification tour.
FEATURES=()
if [[ -n "${LN_FEATURES:-}" ]]; then FEATURES=(--features "$LN_FEATURES"); fi
if [[ "$PROFILE" == "dev" ]]; then
  cargo build -p ln-app ${FEATURES[@]+"${FEATURES[@]}"}; BIN=target/debug/live-notes
else
  cargo build --release -p ln-app ${FEATURES[@]+"${FEATURES[@]}"}; BIN=target/release/live-notes
fi
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/bin"
cp "$BIN" "$APP/Contents/MacOS/live-notes"
sed "s/__VERSION__/$VERSION/g" packaging/Info.plist > "$APP/Contents/Info.plist"
cp packaging/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
if [[ -n "${LN_LLAMA_SERVER:-}" ]]; then
  cp "$LN_LLAMA_SERVER" "$APP/Contents/Resources/bin/llama-server"
fi
IDENTITY="${LN_SIGN_IDENTITY:--}"
codesign --force --deep --options runtime --entitlements packaging/entitlements.plist --sign "$IDENTITY" "$APP"
codesign --verify --strict "$APP"
echo "Built $APP ($VERSION, $PROFILE), signed with: $IDENTITY"
