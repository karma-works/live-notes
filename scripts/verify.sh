#!/usr/bin/env bash
# Development loop for humans and agents (see .claude/skills/verify/SKILL.md).
#
#   scripts/verify.sh quick     fmt, clippy (incl. tour feature), all tests, cargo deny
#   scripts/verify.sh tour      build a dev bundle, drive the app with a scripted tour in an
#                               isolated data dir, write screenshots + tour-report.json
#   scripts/verify.sh probe     headless audio self-test with the real devices (bundle, via open),
#                               plays a speech file so system audio must transcribe it
#   scripts/verify.sh real      tour plus a real recording (mic + system audio) of a played file
#   scripts/verify.sh bench     language detection / transcription on the speech corpus
#   scripts/verify.sh all       quick + tour + probe
#
# Results land in target/verify/<stage>/. Exit code ≠ 0 means a check failed.
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"
OUT="$ROOT/target/verify"
CORPUS="$ROOT/target/test-corpus"
APP="$ROOT/dist/Live Notes.app"
REAL_DATA="$HOME/Library/Application Support/Live Notes"
mkdir -p "$OUT"

corpus() { [[ -f "$CORPUS/demo-others.wav" ]] || scripts/build-test-corpus.sh "$CORPUS" >/dev/null; }

# Models for isolated runs: reuse the real app's or the dev download, never download twice.
models_dir() {
  for d in "$REAL_DATA/models" "$ROOT/.dev-data/models"; do
    [[ -f "$d/whisper-large-v3-turbo-q5_0/ggml-large-v3-turbo-q5_0.bin" ]] && { echo "$d"; return; }
  done
  echo "No models found. Run: LN_DATA_DIR=$ROOT/.dev-data cargo run --release -p ln-bench -- download" >&2
  exit 2
}

bundle() { LN_PROFILE=dev LN_FEATURES=screenshots scripts/bundle-macos.sh | tail -1; }

stage_quick() {
  cargo fmt --all --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo clippy -p ln-app --features screenshots -- -D warnings
  cargo test --workspace 2>&1 | tee "$OUT/tests.log" | grep -E '^test result' | awk '{p+=$4; f+=$6} END {print "tests passed:", p, "failed:", f; exit f>0}'
  command -v cargo-deny >/dev/null && cargo deny check 2>&1 | tail -1
}

run_tour() { # $1 = stage name, extra env as further args
  local name="$1"; shift
  local dir="$OUT/$name" data="$OUT/$name-data"
  rm -rf "$dir" "$data"; mkdir -p "$dir" "$data/data"
  ln -s "$(models_dir)" "$data/data/models"
  corpus
  bundle
  # `open` makes the app bundle the responsible process, so its own permissions apply.
  open -n -W --env LN_DATA_DIR="$data/data" --env LN_NOTES_DIR="$data/notes" --env LN_UI_LANG="${LN_UI_LANG:-de}" \
    --env LN_SCREENSHOT_DIR="$dir" --env LN_DEMO_AUDIO_ME="$CORPUS/demo-me.wav" --env LN_DEMO_AUDIO_OTHERS="$CORPUS/demo-others.wav" \
    "$@" "$APP"
  echo "report: $dir/tour-report.json"
  python3 -I -c "import json,sys; r=json.load(open(sys.argv[1])); [print(('ok  ' if c['ok'] else 'FAIL'), c['check'], c['detail']) for c in r['checks']]; sys.exit(0 if r['passed'] else 1)" "$dir/tour-report.json"
}

stage_tour() { run_tour tour; }
stage_real() { run_tour real --env LN_TOUR_REAL_AUDIO="$CORPUS/mixed.wav"; }

stage_probe() {
  corpus
  bundle
  local report="$OUT/probe.json"
  rm -f "$report"
  (sleep 2; afplay "$CORPUS/mixed.wav") &
  open -n -W "$APP" --args --probe-capture 14 --report "$report"
  wait || true
  python3 -I -c "
import json,sys
r=json.load(open(sys.argv[1]))
print('microphone permission:', r['microphone_permission'], '| vad:', r['vad'], '| transcriber:', r['transcriber'])
for c in r['channels']:
    print(f\"{c['channel']:7} started={c['started']} rate={c['sample_rate']} rms={c['rms']:.4f} nonzero={c['nonzero']:.2f} sections={c['sections']} {c['transcript'][:2]}\")
for f in r['findings']: print('-', f)
ok = all(c['started'] and c['sample_rate']>0 for c in r['channels']) and any(c['channel']=='others' and c['transcript'] for c in r['channels'])
sys.exit(0 if ok else 1)" "$report"
}

stage_bench() {
  corpus
  LN_DATA_DIR="$(dirname "$(models_dir)")" cargo run --release -q -p ln-bench -- asr "$CORPUS"/{de,en,zh,fr,es,mixed}.wav 2>/dev/null | grep -E '^(==|\[|--)'
}

case "${1:-quick}" in
  quick) stage_quick ;;
  tour) stage_tour ;;
  probe) stage_probe ;;
  real) stage_real ;;
  bench) stage_bench ;;
  all) stage_quick && stage_tour && stage_probe ;;
  *) echo "unknown stage $1"; exit 2 ;;
esac
