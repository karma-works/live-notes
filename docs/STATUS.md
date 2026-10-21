# Status v1.0 (Stand 2026-10-21)

Kurzfassung: Alle Funktionen des ersten Durchlaufs (SPEC §16) sind umgesetzt. Seit 2026-10-21
nachmittags prüft der Skill `verify` (`scripts/verify.sh`) die App auch mit **echtem Mikrofon und
System-Audio** und bedient sie wie ein Nutzer (Menübefehle, Kürzel). Offen bleibt die Bewertung
von Zusammenfassung und Live-Antworten mit echtem Gemma (kein Schlüssel vorhanden).

## Fehler aus dem zweiten Praxistest (behoben, mit Prüfung)

| Symptom | Ursache | Prüfung |
|---|---|---|
| Chinesischer Satz wurde zu langem Unsinn („你好你好…“, englischer Fantasietext) | Verkürztes Encoder-Fenster (`audio_ctx`) ließ Whisper bei kurzen Abschnitten in Schleifen fallen; der Wiederholungsfilter kannte nur Wörter mit Leerzeichen | `ln-bench asr target/test-corpus/hard/*.wav` (2,4-s-Satz und verrauschte Fassung korrekt); Tests in `ln-asr/post.rs` (Zeichen-Wiederholungen, Plausibilität Länge/Dauer) |
| UI ~5 s eingefroren bei Start und Stopp | Whisper wurde erst beim Start geladen (GPU-Initialisierung); Stopp wartete auf die Transkription; Whisper nutzte alle Kerne | Whisper wird beim App-Start vorgeladen und von allen Aufnahmen geteilt; Start öffnet die Geräte im Hintergrund („verbindet …“), Stopp kehrt sofort zurück, Transkript und Zusammenfassung laufen sichtbar im Hintergrund, eine neue Aufnahme ist sofort möglich; Tests `stop_is_instant…`, Tour `ui_responsive_*` |
| Zweimal nach „Mikrofon“ gefragt | Die zweite Abfrage ist die macOS-Berechtigung für Systemaudio | Erklärung vor der ersten Aufnahme |
| „Verbindung testen“ mit frisch eingegebenem Schlüssel schlug fehl; falsche OpenRouter-Schlüssel galten als gültig | Test nutzte nur den gespeicherten Schlüssel; OpenRouter `/models` braucht keinen Schlüssel | Test mit dem eingegebenen Schlüssel gegen `/key`; Schlüssel lassen sich entfernen |
| „Für den Assistenten fehlt ein API-Schlüssel (OpenRouter)“ trotz ChatGPT-Anmeldung | Fragen und Vorschläge nutzten weiter OpenRouter | Hinweis nennt die Aufgaben; „ChatGPT-Abo verwenden“; nach der Anmeldung wechseln Aufgaben ohne Schlüssel automatisch |
| Tour las die echten Schlüssel und ChatGPT-Tokens des Nutzers | Tour nutzte den Schlüsselbund | `LN_SECRETS=memory` in `verify.sh` |

## Fehler aus dem ersten Praxistest (behoben, mit Prüfung)

| Symptom | Ursache | Prüfung |
|---|---|---|
| Leeres Transkript | Mikrofon lieferte keine Puffer (AVAudioEngine ohne verbundenen Ausgang; Bluetooth-Headset stoppt die Engine beim Profilwechsel) | `probe`, `real` |
| System-Audio mal still, mal leise | Abgriff am „System-Ausgabegerät“ (virtuelles Proxy-Gerät) statt am Ausgabegerät; Voice-Processing senkte anderen Ton ab; falsche Abtastrate/Kanalzahl (48 kHz gemeldet, 16 kHz tatsächlich) | `probe` (Dauer und Transkript stimmen) |
| Kein Weg zum OpenRouter-Schlüssel | Nach dem Erststart fehlte der Fokus: Menübefehle und Kürzel (⌘, ⌘K ⌘N ⌘E) kamen nicht an | `tour`: `menu_opens_settings`, `shortcut_toggles_editor` |
| Kein Hinweis auf fehlende Schritte | Es gab keine Führung | Checkliste „Noch einrichten“ in der Seitenleiste, Zahnrad für Einstellungen, Hinweise bei stummem Kanal und leerer Aufnahme; `tour`: `setup_lists_missing_key` |
| Keine Diagnose möglich | Kein Logfile | `~/Library/Logs/Live Notes/live-notes.log`; „Aufnahme testen“ in den Einstellungen; `live-notes --probe-capture` |

## Prüfstand

| Prüfung | Ergebnis |
|---|---|
| `scripts/verify.sh quick` | 73 Tests grün, Clippy und fmt sauber, `cargo deny` ok |
| `scripts/verify.sh tour` | 18 Prüfungen der echten App grün: Erststart, Menübefehle, Kürzel, Einstellungen im Hauptfenster, Zurück/Vorwärts, Checkliste (fehlende Schlüssel mit Aufgaben, ChatGPT-Anmeldung), getrennte Rollen, Suche, Aufnahme aus Dateien in 5 Sprachen, Notiz öffnet sofort nach Stopp, **UI ohne Hänger** (längster Stillstand beim Start 110 ms, beim Transkribieren 74 ms, beim Stopp 21 ms) |
| `scripts/verify.sh real` | zusätzlich echte Aufnahme: startet per ⌥⌘R-Aktion, System-Audio wird transkribiert, Mikrofon liefert durchgehend Ton |
| `cargo test --workspace` | 73 Tests grün |
| `cargo clippy --workspace --all-targets -- -D warnings` | keine Befunde (auch mit `--features screenshots`) |
| `cargo fmt --check` | sauber |
| `cargo deny check` | Lizenzen, Sicherheitsmeldungen, Netzwerkregel (§11) ok |
| Echte Modelle (`ln-bench`, M1 8 GB) | Spracherkennung de/en/zh/fr/es je Abschnitt korrekt (Konfidenz 0,96–1,00), Echtzeitfaktor **0,11**, Embeddings sprachübergreifend (Ähnlichkeit 0,48–0,65 verwandt vs. 0,10 fremd) |
| App-Bundle (`scripts/bundle-macos.sh`, Release, signiert ad-hoc + Hardened Runtime) | **37 MB** (Ziel < 40 MB); Arbeitsspeicher im Leerlauf **66 MB** (Ziel < 80 MB) |
| App mit echter Pipeline (Tour, Audiodateien statt Geräten) | Aufnahme → Live-Transkript in 5 Sprachen → Live-Vorschlag → Markdown-Notiz; Screenshots in `docs/screenshots/` |

## Funktionen

| | Funktion | Stand | Geprüft durch |
|---|---|---|---|
| F1 | Aufnahme zwei Kanäle, kein Rohaudio, 10-min-Ringpuffer, Journal, Wiederherstellung | umgesetzt | Tests mit Audiodateien; `probe`/`real` mit echtem Mikrofon (Bluetooth-Headset) und System-Audio. Echo-Unterdrückung startet auf dem Headset im zweiten Versuch; ob sie bei Lautsprecherbetrieb das Gegenüber zuverlässig aus „Ich“ heraushält, ist nicht geprüft |
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
| F7 | Eigenes Modell je Rolle: Fragen, Live-Vorschläge, Zusammenfassung (ältere Einstellungen übernehmen das bisherige Modell) | umgesetzt | Tests, Tour `roles_are_separate` |
| F7 | ChatGPT-Abo per „Sign in with ChatGPT“ (Plan-Nutzung, Responses API) für die Zusammenfassung oder jede andere Rolle | umgesetzt | Anmeldung, Signaturprüfung, Token-Erneuerung, Responses-Stream und Limitfehler gegen Nachbau-Server; Anmelde-URL von OpenAI angenommen („Use ChatGPT to sign in to Live Notes“). **Echte Anmeldung und echte Zusammenfassung über das Abo durch den Nutzer offen** |
| F7 | Monatslimit für Kosten: bei Erreichen pausieren Cloud-Anfragen mit Hinweis, wo man es anhebt (statt Rückfrage-Dialog) | umgesetzt | Test |
| F8 | Live-Vorschläge (Auslöser, Anfrage per Modell bzw. Rückfall, Suche, Kurzantwort, Anheften, Verwerfen) | umgesetzt | Tour (ohne Modell-Schlüssel, also Rückfallweg) |
| F9 | Menüleisten-Symbol, globales Kürzel ⌥⌘R | umgesetzt | App startet stabil; Klicks nicht automatisiert getestet |
| F10 | Markdown lesen (GFM, Tabellen, Code, anklickbare Aufgaben, Transkriptzeilen, leere Abschnitte ausgeblendet) und bearbeiten (Hervorhebung, Autospeichern, Konflikte, Umbenennen nach Überschrift) | umgesetzt | Tests + Screenshots |
| F11 | Kategorien mit macOS-Tag-Farben, Einordnung per Entscheidungsmodell (ein Aufruf pro Notiz; OpenRouter `typesafe/jev-1.14` vor OpenAI `gpt-6-luna`), Farbpunkte in Liste und Kopfzeile, Umbenennen von Hand | umgesetzt | Tests gegen Nachbau-Server (OpenRouter- und OpenAI-Format); **ein echter Aufruf an Jev 1.14 über OpenRouter** (0,7 s, 0,000026 $). OpenAI-Decisions-API nicht gegen das Original geprüft (kein Schlüssel) |
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
- Code-Hervorhebung in Notizen nur für Markdown, Rust, Python, JavaScript und Bash (Größe des
  Bundles); weitere Grammatiken lassen sich in `crates/ln-app/Cargo.toml` zuschalten.
- Xcode ist für den Build nicht nötig (GPUI kompiliert Shader zur Laufzeit); die Installation
  über den App Store wartete beim Abschluss noch auf das Administrator-Passwort.

## Abnahme durch den Projektinhaber

1. `brew install llama.cpp`, dann `cargo run --release -p ln-app` (oder `scripts/bundle-macos.sh`
   und `dist/Live Notes.app` starten).
2. Erster Start: Modelle laden (~1 GB), OpenRouter-Schlüssel eintragen.
3. Videocall mit Kopfhörern und mit Lautsprechern aufnehmen: Kommen beide Kanäle an? Landet beim
   Lautsprecher das Gegenüber nicht doppelt im Kanal „Ich“ (Echo-Unterdrückung)?
4. Berechtigungsdialoge für Mikrofon und System-Audio prüfen.
5. Qualität von Zusammenfassung und Live-Vorschlägen mit echtem Gemma bewerten.
6. Optional: OpenAI-Schlüssel eintragen und „OpenAI günstig“ testen.

## Entwicklungsschleife

Skill `.claude/skills/verify/SKILL.md`, Befehl `scripts/verify.sh quick|tour|probe|real|bench|all`.

## Wichtige Befehle

```bash
cargo test --workspace
cargo run --release -p ln-app
LN_DATA_DIR=$PWD/.dev-data cargo run --release -p ln-bench -- download
LN_DATA_DIR=$PWD/.dev-data cargo run --release -p ln-bench -- asr target/test-corpus/*.wav
cargo run -p ln-app --features screenshots   # mit LN_SCREENSHOT_DIR=… rendert die Tour
```
