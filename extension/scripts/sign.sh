#!/usr/bin/env bash
# Baut die Extension und lässt sie von addons.mozilla.org als "unlisted" signieren.
# Erwartet WEB_EXT_API_KEY und WEB_EXT_API_SECRET in der Umgebung; web-ext liest sie selbst,
# sie werden weder auf der Kommandozeile übergeben noch irgendwo gespeichert.
set -euo pipefail

cd "$(dirname "$0")/.."

# Optional: Schlüssel aus extension/.amo-credentials (nicht im Git, nur für dich lesbar).
if [[ -f .amo-credentials ]]; then
  set -a
  # shellcheck disable=SC1091
  source .amo-credentials
  set +a
fi

if [[ -z "${WEB_EXT_API_KEY:-}" || -z "${WEB_EXT_API_SECRET:-}" ]]; then
  echo "WEB_EXT_API_KEY und WEB_EXT_API_SECRET fehlen (Umgebung oder extension/.amo-credentials)." >&2
  echo "Schlüssel gibt es unter https://addons.mozilla.org/developers/addon/api/key/" >&2
  exit 1
fi

[[ -d node_modules ]] || npm ci
node scripts/build.mjs
npx web-ext lint --source-dir dist
npx web-ext sign \
  --source-dir dist \
  --artifacts-dir web-ext-artifacts \
  --channel unlisted

echo
echo "Signierte .xpi:"
ls -1t web-ext-artifacts/*.xpi | head -n 1
