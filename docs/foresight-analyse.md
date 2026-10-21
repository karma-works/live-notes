# Analyse: Google AI Edge Foresight (macOS, v1.0.0, Build 1.0.0.28.20261005)

Quelle: `foresight_mac.dmg` im Projektverzeichnis. Untersucht wurden Bundle-Struktur, `Info.plist`,
Flutter-Assets und die Strings der Binaries (Haupt-App, `ForesightHelper`, LiteRT-Dylibs).
Es wurde nichts ausgeführt. Aussagen, die nur aus Strings abgeleitet sind, sind als **Indiz** markiert.

## 1. Technischer Aufbau

| Bereich | Befund |
|---|---|
| UI | Flutter (FlutterMacOS.framework, Dart AOT), Skia-Renderer (Impeller aus), Material-UI, Google-Sans-Fonts |
| Bundle-Größe | ca. 376 MB (Binary 143 MB, Frameworks 105 MB, Resources 128 MB) – ohne Modelle |
| ML-Runtime | Google **LiteRT / LiteRT-LM** (`libLiteRt*.dylib`, Metal- und WebGPU-Accelerator), MediaPipe-Graphen |
| Hintergrundprozess | `ForesightHelper` (90 MB, Launch Agent) – Indexierung/Hintergrundaufgaben |
| Mindest-OS | macOS 14.2 (Grund: Core Audio **Process Tap** für System-Audio) |
| Updater | Google Keystone / GoogleUpdater |
| Telemetrie/Cloud | Firebase (Installations, Messaging, App Check, Remote Config/Experimente), Google Sign-In, `odml.pa.googleapis.com` |
| Lokalisierung | UI in sehr vielen Sprachen (l10n-Dateien), Ansage-Sounds pro Sprache |

## 2. Modelle

Aus `lib/assets/on_device_model_configure.json` – Auswahl nach Gerätespeicher:

| Modell | Mindest-RAM | Audio-Backend | Vision-Backend | Kontext | Sampling |
|---|---|---|---|---|---|
| Gemma 4 E2B | 6 GB | CPU | GPU | 4096 | T=0.4, topK=40, topP=0.9, RepPen 1.05 |
| Gemma 4 E4B | 15 GB | CPU | GPU | 4096 | wie oben |
| Gemma 4 12B | 30 GB | GPU | GPU | 4096 | wie oben |

Alle mit spekulativem Decoding (Drafter), Thinking aus, Vision- und Audio-Eingang.

Zusätzlich „Expert models“ (werden mit dem Gemma-Modell geladen):

- **EmbeddingGemma** (`EmbeddingGemmaV2`, UI-Text „Indexed with local EmbeddingGemma“) für die Vektorsuche.
- **ASR auf USM-Basis**: Modelltypen wie `GEMMA4_200M_USM_75M_TOKENIZER_EN_16K`,
  `GEMMA4_42M_USM_24M_TOKENIZER_EN_16K`, `USM_NANOV2_700M_CAUSAL`, `MODEL_TYPE_ASR_MULTILINGUAL`.
  Zusätzlich ein `Whisper`-Protoeintrag in LiteRT-LM.
- **VAD**: RNNoise/WebRTC-RNN-VAD, `VadSegmenter` mit Hysterese-Glättung.
- **OpenCC + cppjieba**: Umwandlung traditionelles ↔ vereinfachtes Chinesisch, Wortsegmentierung.
- „Lightweight query detection“ für den Live-Assistenten (Frage im Gespräch erkennen).

**Indiz für die Sprach-Einschränkung:** Die ausgelieferten ASR-Varianten tragen `TOKENIZER_EN_16K` im Namen,
und es gibt die Meldung `Language ID is not supported for kGemma4 models.` Die Transkription läuft also
offenbar mit festem Englisch-Tokenizer bzw. ohne Spracherkennung. Das deckt sich mit der beobachteten
Schwäche bei Chinesisch/Deutsch.

## 3. Audio-Aufnahme

- System-Audio: `AudioHardwareCreateProcessTap` (Core Audio Taps, macOS 14.2+), Berechtigung
  `NSAudioCaptureUsageDescription`.
- Mikrofon: `AVAudioEngine` mit **Voice Processing** (Echo-Unterdrückung/AEC, AGC, Ducking-Konfiguration).
- Zwei getrennte Kanäle mit Labels **„Microphone“** und **„Remote“**. Die Prompts verbieten dem LLM,
  diese Labels als Sprechernamen zu verwenden.
- Rechtlicher Hinweis vor dem Start: Zustimmung aller Sprecher, Ansage am Anfang, Erinnerungston alle 30 Minuten.

## 4. Index & Suche

- SQLite mit **FTS5** (Volltext, BM25) und Vektortabellen (`vec0`, ScaNN-Strings), Fusion per **RRF**
  (Reciprocal Rank Fusion) → hybride Suche.
- Tabelle `documents(id, content, userId, maxTokensPerChunk, chunkOverlap, mimeType, imageBytes)`.
- Chunking mit konfigurierbarer Chunkgröße und Überlappung pro Dokument.
- Dateien: PDF, Markdown, Text, DOCX, Bilder, Audio, Video (Icons im Bundle), dazu Google Docs/Sheets/Slides.
- „Import folder“, „Include subfolders“, „Knowledge Sources“, „Manage sources“.

## 5. Funktionen (aus UI-Strings und eingebauter Hilfe)

- **Knowledge Hub**: lokale Dateien + Google Drive einbinden.
- **Meeting-Transkription** mit „Full Transcript“, „Meeting Transcript Summary“, Export, Kopieren.
- **Enhanced Notes / Live Enhance**: eigene Stichpunkte während des Meetings werden mit dem Transkript
  angereichert; „Potential Important Topics Not In Notes“; einstellbarer „Default Note Enhancement Style“.
- **Action Items** mit Zuweisung an Sprecher.
- **Ask Foresight**: Chat über Wissen und Meetings (optional mit Websuche über Google Custom Search).
- **Live-Assistent / HUD**: erkennt Fragen im Gespräch und zeigt passende Antworten.
- **Daily Briefings / Meeting Brief**: Vorbereitung anhand von Google-Kalender-Terminen.
- **Labels / Wiki**: Dokumente semantisch gruppieren, Wiki-Zusammenfassung, Widersprüche erkennen.
- **Onboarding**: mehrere Intro-Seiten, Google-Anmeldung, Modellauswahl („Choose your Gemma model“),
  „Grant All Permissions“, Download der Expert-Modelle.
- „Delete all user data and models“.

## 6. Was wir übernehmen, was nicht

| Übernehmen | Weglassen |
|---|---|
| Gemma-4-Familie mit RAM-abhängiger Auswahl | Google-Konto, Drive, Docs, Kalender, Custom Search |
| EmbeddingGemma für mehrsprachige Vektorsuche | Firebase, Telemetrie, Remote-Experimente, Keystone |
| Process Tap + Mic mit AEC, zwei Kanäle | Mehrseitiges Onboarding, „Grant All Permissions“-Seite |
| Hybride Suche FTS5 + Vektor + RRF in SQLite | Flutter/Skia, separater 90-MB-Helper |
| VAD-Segmentierung, OpenCC für Chinesisch | Englisch-only-ASR |
| Notiz-Anreicherung, Action Items, Zustimmungs-Hinweis | Daily Briefings (kalenderbasiert) – ggf. später mit lokalem Kalender |
