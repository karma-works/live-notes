---
name: verify
description: Development and verification loop for Live Notes. Use after every change to the app, before reporting work as done, and when the user reports that something "does not work" (recording, transcript, summary, keys, menus). Builds, lints, tests, drives the real app with a scripted tour (menu actions dispatched like a user), checks real microphone and system audio, and reads the logs. Never report success without a passing run of the relevant stages.
---

# Verify Live Notes

Everything runs through `scripts/verify.sh <stage>`. Results land in `target/verify/`.
A stage exits non-zero when a check fails. Read the report, don't guess.

| Stage | What it proves | Time |
|---|---|---|
| `quick` | `cargo fmt --check`, clippy `-D warnings` (also with the `screenshots` feature), all tests, `cargo deny` | ~2 min |
| `tour` | The real app, started as a bundle via `open`, in an isolated data dir: first-run screen, **menu/shortcut actions reach the view** (settings, ⌘E, ⌘K), setup checklist names missing keys, search finds notes, a two-channel recording from WAV files yields a 5-language transcript that lands in the Markdown note. Screenshots of every view. | ~2 min |
| `probe` | Headless self-test with the **real microphone and system audio** (`live-notes --probe-capture`). `afplay` plays a speech file meanwhile, so system audio must transcribe something. | ~1 min |
| `real` | `tour` + a real recording started by the ⌥⌘R action while `afplay` plays speech: system audio must transcribe, the microphone must deliver. Plays ~40 s of audio through the speakers/headset. | ~3 min |
| `bench` | Language detection and transcription on the speech corpus (`ln-bench asr`), with real-time factor. | ~1 min |
| `all` | `quick` + `tour` + `probe` | |

## The loop

1. Change code.
2. `scripts/verify.sh quick` — fix every finding.
3. UI or engine change → `scripts/verify.sh tour`, then **look at the screenshots** in
   `target/verify/tour/*.png` with the Read tool. Check the report:
   `target/verify/tour/tour-report.json` (`"passed": true`, every check `ok`).
4. Audio change, or a user reports "no transcript" → `scripts/verify.sh probe`, then `real`.
5. Read the log of the run: `target/verify/<stage>-data/data/logs/live-notes.log`
   (the user's real app logs to `~/Library/Logs/Live Notes/live-notes.log`).
6. Only report done when the relevant stages pass. Say which stages ran and what they could
   not cover.

## Reading the probe report (`target/verify/probe.json`)

Per channel (`me` = microphone, `others` = system audio):

| Field | Meaning | Typical fault |
|---|---|---|
| `started=false` / `error` | the device could not be opened | permission, device busy |
| `sample_rate=0` | no buffers at all | AVAudioEngine stalled (Bluetooth profile switch), engine not pulled |
| `nonzero≈0` | only digital silence | permission missing, nothing playing, tap bound to the wrong device |
| `seconds` ≪ probe length | wrong sample rate or channel layout | rate taken from the tap format instead of the aggregate |
| `sections=0` with `rms>0.01` | sound arrives but no speech detected | wrong rate (speech sped up), VAD |
| `transcript` | what Whisper understood, per section with language | |

## Known device behaviour (keep these fixed)

All found with `probe`/`real` on a MacBook Air M1 with a Bluetooth headset:

- AVAudioEngine **without voice processing** only delivers buffers if mixer → output is
  connected (`mainMixerNode`).
- Opening a Bluetooth headset's mic switches it to its 16 kHz call profile; the first engine
  start then gets no buffers. → retry, and restart on stalls (`ln-platform/src/macos/mic.rs`).
- Voice processing ducks all other audio, which also silences the system-audio tap. → minimum
  ducking.
- Bind the tap's aggregate device to `kAudioHardwarePropertyDefaultOutputDevice`, **not** the
  "system output device" (alert sounds; can be a virtual proxy device).
- Use the **aggregate's** nominal sample rate and each buffer's `mNumberChannels`; the tap
  format can say 48 kHz stereo while the device runs at 16 kHz.
- Rebuild the tap when the output device or its sample rate changes.
- GPUI dispatches actions inside a window update: app-level fallback handlers must `cx.defer`
  before updating the window, and the main view must get the focus back after the first-run
  screen.

## Permissions

`probe` and `real` open the app bundle with `open`, so the **app's** permissions apply
(microphone; Screen & System Audio Recording → System Audio Only). Ad-hoc signatures change on
every build, so macOS may ask again; the prompt appears on the user's screen. If a stage reports
`microphone_permission: denied`, ask the user to allow Live Notes in System Settings → Privacy &
Security, then rerun.

## Prerequisites

- Models: `~/Library/Application Support/Live Notes/models` or
  `LN_DATA_DIR=$PWD/.dev-data cargo run --release -p ln-bench -- download`.
- `llama-server` for embeddings: `brew install llama.cpp`.
- Speech corpus: built automatically by `scripts/build-test-corpus.sh` (macOS voices).

## Extending the tour

Checks live in `crates/ln-app/src/tour.rs` (feature `screenshots`). Drive the UI through
`dispatch(cx, handle, Action)` — the same path as menus and shortcuts — and assert outcomes with
`r.check(name, ok, detail)`. Add a check for every bug a user reports before fixing it, so the
tour fails first and passes after the fix.
