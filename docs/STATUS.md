# Status v1.0 (Stand 2026-10-21)

Kurzfassung: Alle Funktionen des ersten Durchlaufs (SPEC §16) sind umgesetzt, kompilieren ohne
Warnungen und sind mit Tests oder echten Modellen geprüft. **Nicht prüfen konnte der Agent die
echte Aufnahme über Mikrofon und System-Audio** – das ist die Abnahme durch den Projektinhaber.

## Prüfstand

| Prüfung | Ergebnis |
|---|---|
| `cargo test --workspace` | 58 Tests grün |
| `cargo clippy --workspace --all-targets -- -D warnings` | keine Befunde (auch mit `--features screenshots`) |
| `cargo fmt --check` | sauber |
| `cargo deny check` | Lizenzen, Sicherheitsmeldungen, Netzwerkregel (§11) ok |
| Echte Modelle (`ln-bench`, M1 8 GB) | Spracherkennung de/en/zh/fr/es je Abschnitt korrekt (Konfidenz 0,96–1,00), Echtzeitfaktor **0,11**, Embeddings sprachübergreifend (Ähnlichkeit 0,48–0,65 verwandt vs. 0,10 fremd) |
| App mit echter Pipeline (Tour, Audiodateien statt Geräten) | Aufnahme → Live-Transkript in 5 Sprachen → Live-Vorschlag → Markdown-Notiz; Screenshots in `docs/screenshots/` |

## Funktionen

| | Funktion | Stand | Geprüft durch |
|---|---|---|---|
| F1 | Aufnahme zwei Kanäle, kein Rohaudio, 10-min-Ringpuffer, Journal, Wiederherstellung | umgesetzt | Tests mit Audiodateien; **Mikrofon (AVAudioEngine + Echo-Unterdrückung) und System-Audio (Process Tap) nur kompiliert, nicht live getestet** |
| F2 | Sprachrichtige Transkription: VAD (Silero), Spracherkennung (Whisper base) innerhalb „Meine Sprachen“, Whisper large-v3-turbo, OpenCC, Halluzinationsfilter, Sprachkorrektur | umgesetzt | Tests + echte Modelle |
| F2 | OpenAI „günstig“ (`gpt-transcribe`) mit Rückfall auf lokal | umgesetzt | gegen Nachbau-Server; **nicht gegen die echte OpenAI-API** |
| F2 | OpenAI „live“ (`gpt-live-transcribe`, Realtime-API) | v1.1 – der Modus nutzt bis dahin denselben REST-Weg wie „günstig“ | – |
| F3 | Markdown-Notizen, automatische Zusammenfassung, Aufgaben, Titel + Umbenennung, Ergänzungen als „Vorschlag“ | umgesetzt | Tests gegen Nachbau-Server; **echte Gemma-Antworten über OpenRouter ungetestet (kein Schlüssel)** |
| F4 | Quellen: Scan mit `.gitignore`, FSEvents, inkrementell, Formate md/txt/Code/HTML/PDF/DOCX/PPTX/ODT/XLSX/RTF/iWork/EML, Bilder per Apple-Vision-OCR | umgesetzt | Tests; PDF/OCR nur kompiliert |
| F4 | Audio-/Videodateien in Quellen transkribieren | offen (werden übersprungen) | – |
| F4 | Bildbeschreibung mit Gemma | v1.1 laut §16 | – |
| F5 | Hybride Suche (FTS5 + Trigram für CJK + sqlite-vec int8 + RRF), schwache Bedeutungstreffer gefiltert | umgesetzt | Tests + echte Embeddings |
| F6 | Fragen mit nummerierten Quellen, Streaming, Folgefragen, „laufendes Gespräch“ | umgesetzt | Test gegen Nachbau-Server |
| F7 | Einstellungen (alle Abschnitte), Schlüssel im Schlüsselbund, Verbindung testen | umgesetzt | Screenshot; Schlüsselbund-Zugriff nur kompiliert |
| F7 | Monatslimit für Kosten: bei Erreichen pausieren Cloud-Anfragen mit Hinweis, wo man es anhebt (statt Rückfrage-Dialog) | umgesetzt | Test |
| F8 | Live-Vorschläge (Auslöser, Anfrage per Modell bzw. Rückfall, Suche, Kurzantwort, Anheften, Verwerfen) | umgesetzt | Tour (ohne Modell-Schlüssel, also Rückfallweg) |
| F9 | Menüleisten-Symbol, globales Kürzel ⌥⌘R | umgesetzt | App startet stabil; Klicks nicht automatisiert getestet |
| F10 | Markdown lesen (GFM, Tabellen, Code, anklickbare Aufgaben, Transkriptzeilen, leere Abschnitte ausgeblendet) und bearbeiten (Hervorhebung, Autospeichern, Konflikte, Umbenennen nach Überschrift) | umgesetzt | Tests + Screenshots |
| §4 | Ein-Bildschirm-Start mit Download im Hintergrund, Fortsetzen nach Abbruch, SHA-256 | umgesetzt | Tests + echter Download |

## Abweichungen von der Spezifikation

- **Zusätzliches Modell:** Whisper base (82 MB) übernimmt nur die Spracherkennung. Ohne es kostet
  jeder Abschnitt zwei volle Whisper-Encoder-Läufe (Echtzeitfaktor 0,65 statt 0,11).
- **Audiokontext pro Abschnitt:** Whisper rechnet nur so viel Fenster, wie der Abschnitt lang ist.
- **`llama-server`** (für EmbeddingGemma und lokales Gemma) kommt in v1.0 aus Homebrew
  (`brew install llama.cpp`) bzw. `$LN_LLAMA_SERVER`; das Bundle enthält es noch nicht, weil die
  Homebrew-Variante viele dynamische Bibliotheken nachlädt.
- **Signierung:** ad-hoc (`scripts/bundle-macos.sh`). Mit `LN_SIGN_IDENTITY` lässt sich ein
  stabiles Zertifikat nutzen; macOS fragt bei ad-hoc-Signaturen nach jedem Build erneut nach
  Mikrofon/Audio.
- **Testkorpus:** synthetische macOS-Stimmen statt FLEURS (`scripts/build-test-corpus.sh`).
  Echte Sprecher und Telefonqualität fehlen noch; die Werte sind daher eine Obergrenze.
- **Crates:** zusätzlich `ln-types` (gemeinsame Typen) und `ln-bench` (Messwerkzeug); die Fassade
  heißt wie geplant `ln-core`.

## Bekannte Schwächen

- Chinesisch mit synthetischer Stimme: einzelne Verhörer („日算“ statt „预算“).
- Treffer-Ausschnitte zeigen rohes Markdown (z. B. `- [ ]`).
- Der Zeitstempel des ersten Abschnitts kann wegen der VAD-Polsterung `00:00:00` sein.

## Abnahme durch den Projektinhaber

1. `brew install llama.cpp`, dann `cargo run --release -p ln-app` (oder `scripts/bundle-macos.sh`
   und `dist/Live Notes.app` starten).
2. Erster Start: Modelle laden (~1 GB), OpenRouter-Schlüssel eintragen.
3. Videocall mit Kopfhörern und mit Lautsprechern aufnehmen: Kommen beide Kanäle an? Landet beim
   Lautsprecher das Gegenüber nicht doppelt im Kanal „Ich“ (Echo-Unterdrückung)?
4. Berechtigungsdialoge für Mikrofon und System-Audio prüfen.
5. Qualität von Zusammenfassung und Live-Vorschlägen mit echtem Gemma bewerten.
6. Optional: OpenAI-Schlüssel eintragen und „OpenAI günstig“ testen.

## Wichtige Befehle

```bash
cargo test --workspace
cargo run --release -p ln-app
LN_DATA_DIR=$PWD/.dev-data cargo run --release -p ln-bench -- download
LN_DATA_DIR=$PWD/.dev-data cargo run --release -p ln-bench -- asr target/test-corpus/*.wav
cargo run -p ln-app --features screenshots   # mit LN_SCREENSHOT_DIR=… rendert die Tour
```
