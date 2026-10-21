#!/usr/bin/env bash
# Builds a small multilingual speech corpus with the macOS voices (no download needed):
# one WAV per main language plus a mixed conversation, 16 kHz mono.
#
#   scripts/build-test-corpus.sh [out-dir]       default: target/test-corpus
#   LN_DATA_DIR=… cargo run --release -p ln-bench -- asr target/test-corpus/*.wav
#
# FLEURS (CC-BY-4.0) with real speakers is the planned extension (SPEC §16); synthetic
# voices are cleaner than real calls, so treat these numbers as an upper bound.
set -euo pipefail
OUT="${1:-target/test-corpus}"
mkdir -p "$OUT"
cd "$OUT"
say -v "Flo (German (Germany))" -o de.aiff "Können wir das Budget für das erste Quartal bis Freitag festlegen? Ich schicke dir morgen die Zahlen."
say -v "Flo (English (US))" -o en.aiff "Could you send me the updated offer by Friday? We still need to agree on the delivery time."
say -v "Flo (Chinese (China mainland))" -o zh.aiff "我们下周再讨论预算，我周五前把报价发给你。交货时间还需要确认。"
say -v "Flo (French (France))" -o fr.aiff "Le budget du prochain trimestre sera discuté la semaine prochaine. Pouvez-vous m'envoyer le contrat?"
say -v "Flo (Spanish (Spain))" -o es.aiff "El presupuesto se discutirá la próxima semana. ¿Puedes enviarme la oferta antes del viernes?"
for l in de en zh fr es; do afconvert -f WAVE -d LEI16@16000 -c 1 "$l.aiff" "$l.wav" && rm "$l.aiff"; done
python3 -I - <<'PY'
import wave
def rd(n):
    w = wave.open(n); d = w.readframes(w.getnframes()); w.close(); return d
sil = b"\x00\x00" * 16000
out = sil + rd("de.wav") + sil + rd("zh.wav") + sil + rd("fr.wav") + sil + rd("en.wav") + sil + rd("es.wav") + sil
w = wave.open("mixed.wav", "wb"); w.setnchannels(1); w.setsampwidth(2); w.setframerate(16000); w.writeframes(out); w.close()
PY
echo "Corpus in $OUT"
