# Live Notes – Spezifikation v0.1 (Entwurf)

Status: Entwurf zur Diskussion · Stand 2026-10-21 · Arbeitstitel „Live Notes“

Begleitdokumente:
- [Foresight-Analyse](foresight-analyse.md)
- [ADR-0001: UI-Framework](adr/ADR-0001-ui-framework.md)

---

## 1. Ziel

Live Notes ist eine native macOS-App in Rust, die vollständig lokal

1. **Gespräche mitschneidet** (Mikrofon + System-Audio) und als **sprachrichtiges Transkript**
   in einer Notizdatei speichert,
2. **lokale Verzeichnisse indexiert** und in einer Vektordatenbank durchsuchbar macht,
3. **Fragen beantwortet** über Notizen und Dateien (RAG mit Quellenangaben).

Vorbild ist Google AI Edge Foresight. Unterschiede:

| | Foresight | Live Notes |
|---|---|---|
| UI | Flutter | native Rust-UI (GPUI, Metal) |
| Transkription | faktisch englischzentriert | **in der gesprochenen Sprache**, je Abschnitt erkannt |
| Onboarding | mehrseitiger Wizard, Google-Login | **ein Bildschirm**: Modell-Download bestätigen |
| Cloud | Google Drive, Docs, Kalender, Firebase | **keine**; Netzwerk nur für Modell-Download |
| Speicherort | interne Datenbank | **Markdown-Dateien** in einem sichtbaren Ordner + Index |
| Größe | ~376 MB App + Helper | Ziel < 40 MB App (ohne Modelle) |

## 2. Nicht-Ziele

- Keine Google-, Microsoft- oder sonstige Cloud-Integration (Drive, Docs, Kalender, Login).
- Keine Telemetrie, kein Crash-Upload, keine Remote-Konfiguration.
- Keine Websuche.
- Kein Konto, keine Synchronisation zwischen Geräten (Nutzer kann den Notizordner selbst syncen).
- Keine Aktionen nach außen (keine E-Mails, keine Dateiänderungen außerhalb des Notizordners).
- v1 nur macOS auf Apple Silicon.

## 3. Plattform und Rahmenbedingungen

- macOS **14.2+** (Core Audio Process Tap), Apple Silicon (M1+). Intel wird nicht unterstützt.
- Referenzgerät für Budgets: **MacBook Air M1, 8 GB RAM** (Gerät des Projektinhabers).
- Signiert und notarisiert, Hardened Runtime. App Sandbox in v1 **nicht**, weil beliebige
  Quellordner und Process Taps sonst umständlich werden (Security-Scoped Bookmarks bleiben trotzdem
  vorbereitet, falls wir später in den App Store wollen).

## 4. Erster Start (statt Onboarding-Wizard)

Ein einziger Bildschirm:

```
┌───────────────────────────────────────────────────────────┐
│  Live Notes                                               │
│                                                           │
│  Für die lokale KI werden Modelle geladen (einmalig).     │
│                                                           │
│  Empfohlen für diesen Mac (8 GB RAM):                     │
│   • Transkription   Whisper large-v3-turbo   ~ 0,6 GB     │
│   • Suche           EmbeddingGemma 300M      ~ 0,3 GB     │
│   • Assistent       Gemma 4 E2B              ~ 3 GB       │
│                                       Gesamt ~ 3,9 GB     │
│  Speicherort: ~/Library/Application Support/Live Notes    │
│  [ Andere Auswahl … ]                                     │
│                                                           │
│                        [ Herunterladen und starten ]      │
└───────────────────────────────────────────────────────────┘
```

- Nach dem Klick landet man **sofort in der App**. Der Download läuft im Hintergrund mit
  Fortschrittsanzeige in der Seitenleiste. Funktionen werden freigeschaltet, sobald ihr Modell da ist
  (Reihenfolge: Embedding → ASR → LLM, kleinste zuerst).
- Quellordner können sofort hinzugefügt werden; die Indexierung startet, sobald das Embedding-Modell
  da ist (vorher läuft schon Parsing + Volltextindex).
- **Berechtigungen werden erst bei Bedarf** abgefragt: Mikrofon und System-Audio beim ersten Start
  einer Aufnahme, Ordnerzugriff über den Dateidialog.
- Ein leerer Zustand im Hauptfenster zeigt zwei Aktionen: „Ordner hinzufügen“ und „Aufnahme starten“.
- Optional „Modelle aus Ordner importieren“ (Offline-Installation, z. B. für Firmen).

## 5. Funktionen

### F1 Aufnahme

- Start/Stopp über: Aufnahme-Knopf, Menüleisten-Icon, globales Tastenkürzel (Standard `⌥⌘R`).
- **Zwei Kanäle**:
  - `ich`: Mikrofon über Voice-Processing-I/O (Echo-Unterdrückung, damit Lautsprecher-Audio nicht
    doppelt im Mikrofon landet).
  - `andere`: System-Audio über Core Audio Process Tap. Standard: alle Prozesse außer Live Notes.
    Optional nur bestimmte Apps (Zoom, Teams, Browser).
- Auswahl Eingabegerät, Pegelanzeige je Kanal, Stummschalten je Kanal.
- Pausieren/Fortsetzen; eine Aufnahme = eine Notiz.
- Rohaudio optional speichern (Einstellung, Standard **an**): Stereo-AAC (`.m4a`, L = ich, R = andere),
  ca. 0,5 MB/min. Löschbar pro Notiz; automatische Löschung nach N Tagen einstellbar.
- Zustimmungshinweis vor der ersten Aufnahme (einmalig bestätigen): Alle Teilnehmenden müssen
  einverstanden sein. Optional Erinnerungston alle 30 Minuten.
- Absturzsicherheit: Audio und Transkript werden laufend geschrieben (Segment-Journal), nach einem
  Absturz wird die Notiz beim nächsten Start wiederhergestellt.
- Idee: **Meeting-Erkennung** – wenn Zoom/Teams/FaceTime/ein Browser-Call Audio-Ein- und -Ausgabe
  gleichzeitig nutzt, erscheint eine dezente Mitteilung „Aufnahme starten?“ (abschaltbar).

### F2 Sprachrichtige Transkription (Kernunterschied)

Anforderung: Was auf Chinesisch gesagt wird, wird auf Chinesisch transkribiert, Deutsch auf Deutsch usw.
Keine Übersetzung.

Ablauf je Kanal:

1. Resampling auf 16 kHz mono.
2. **VAD** (Silero VAD) schneidet Sprachabschnitte (0,5–25 s, Schnitt bevorzugt in Pausen).
3. **Spracherkennung pro Abschnitt**: Whisper liefert Sprachwahrscheinlichkeiten. Wir wählen die
   wahrscheinlichste Sprache **innerhalb der vom Nutzer gewählten Sprachen** („Meine Sprachen“,
   Standard: Systemsprachen, z. B. Deutsch + Englisch + Chinesisch). Das verhindert
   Fehlklassifizierungen (z. B. Niederländisch statt Deutsch).
4. **Transkription mit `task = transcribe`** und fest gesetzter erkannter Sprache. Nie `translate`.
5. Glättung: Ist die Erkennung unsicher (< 0,6) und der Abschnitt kurz (< 2 s), wird die Sprache des
   vorigen Abschnitts desselben Kanals übernommen.
6. Kontext-Prompt (`initial_prompt`) nur aus vorherigen Abschnitten **derselben Sprache**, sonst
   „zieht“ Whisper in die falsche Sprache.
7. Nachbearbeitung je Sprache:
   - Chinesisch: **OpenCC** auf die gewünschte Schrift (vereinfacht/traditionell, Einstellung), da
     Whisper beides mischt. Satzzeichen auf chinesische Vollbreite normalisieren.
   - Alle: Halluzinationsfilter (Wiederholungen, typische Phrasen wie „Untertitel von …“ bei Stille,
     `no_speech_prob`-Schwelle).
8. Manuelle Korrektur: Sprache eines Abschnitts per Klick ändern → Abschnitt wird neu transkribiert.
   Pro Notiz kann eine Sprache fest vorgegeben werden („Ganze Aufnahme ist Chinesisch“).

Darstellung: Jede Zeile trägt Zeitstempel, Kanal und Sprachkürzel, z. B.
`[00:12:03] Andere (zh): 我们下周再讨论预算。`

Latenz-Ziel: Text erscheint ≤ 3 s nach Ende eines Abschnitts (M1, large-v3-turbo, Metal).
Bei Rückstau: Warteschlange, nichts geht verloren; nach Aufnahmeende wird fertig transkribiert.

Mehrsprachigkeit innerhalb eines Satzes (Code-Switching, z. B. deutsche Sätze mit englischen
Fachbegriffen) bleibt in der Hauptsprache des Abschnitts – das macht Whisper ordentlich.

### F3 Notizen

- Jede Aufnahme erzeugt eine **Markdown-Datei** (Format siehe §8.1). Auch leere Notizen ohne Aufnahme
  sind möglich (eigene Stichpunkte).
- Ansicht: links eigene Notizen (editierbar), rechts bzw. darunter das Live-Transkript.
- Nach der Aufnahme automatisch (abschaltbar), mit dem LLM:
  - **Titel** (in der Hauptsprache des Gesprächs),
  - **Zusammenfassung**, **Entscheidungen**, **Aufgaben** (mit Zuständigen, falls Namen fallen;
    „ich“/„andere“ werden nie als Namen verwendet – wie bei Foresight),
  - **Notizen anreichern**: eigene Stichpunkte werden mit Inhalten aus dem Transkript ergänzt,
    Ergänzungen sind farblich markiert und einzeln annehmbar/verwerfbar.
- Sprache der Zusammenfassung: Einstellung „wie das Gespräch“ (Standard) oder feste Sprache.
- Sprecher umbenennen: „andere“ → „Frau Li“ (nur in dieser Notiz). Echte Sprechertrennung innerhalb
  eines Kanals ist v2 (§12).
- Export: Markdown (ist schon die Datei), Text, Kopieren in Zwischenablage.
- Notizliste mit Suche, Filter nach Datum/Sprache/Label, Sortierung.

### F4 Quellen und Indexierung

- Ordner hinzufügen per Dateidialog oder Drag & Drop; Unterordner standardmäßig an.
- Der Notizordner ist immer eine Quelle.
- Pro Quelle: Status (indexiert / läuft / Fehler), Anzahl Dateien, Größe, letzte Aktualisierung,
  Pausieren, Entfernen (entfernt nur Indexdaten, nie Dateien).
- Ausschlüsse: `.gitignore`/`.ignore` werden respektiert, Standard-Ausschlüsse (`node_modules`,
  `.git`, `target`, `Library`, Dateien > 50 MB), eigene Glob-Muster.
- Live-Aktualisierung über **FSEvents**; beim Start Abgleich über mtime + Größe, bei Änderung
  Inhalts-Hash (BLAKE3) → nur geänderte Dateien neu verarbeiten.
- Indexierung läuft mit niedriger Priorität (QoS utility), pausiert bei Akku < 20 % und während einer
  Aufnahme (Transkription hat Vorrang).
- Unterstützte Formate v1:

| Format | Extraktion |
|---|---|
| `.md`, `.txt`, Quellcode, `.csv`, `.json`, `.yaml` | direkt (UTF-8, Encoding-Erkennung) |
| `.pdf` | PDFKit (nativ); Seiten ohne Text → OCR |
| `.docx`, `.pptx`, `.xlsx`, `.odt` | ZIP+XML-Parser (`calamine` für Tabellen) |
| `.rtf`, `.html` | NSAttributedString bzw. `html2text` |
| `.pages`, `.key`, `.numbers` | Vorschau-PDF im Paket, sonst überspringen |
| `.png`, `.jpg`, `.heic` | **Apple Vision OCR** (mehrsprachig inkl. Chinesisch) |
| `.m4a`, `.mp3`, `.wav`, `.mp4`, `.mov` | Transkription wie F2 (Hintergrund, abschaltbar) |
| `.eml` | Header + Text |

### F5 Suche

- Ein Suchfeld (`⌘K`) über Notizen und Dateien.
- **Hybrid**: FTS5 (BM25) + Vektorsuche, zusammengeführt mit RRF (k = 60), danach Gruppierung pro Datei.
- **Sprachübergreifend**: EmbeddingGemma ist mehrsprachig – „Budget nächstes Quartal“ findet auch
  eine chinesische Notiz über „下季度预算“.
- FTS5-Tokenizer: `unicode61` für Alphabetschriften, für CJK zusätzlich Bigramme (oder `trigram`),
  damit Chinesisch ohne Leerzeichen gefunden wird.
- Ergebnis: Titel, Pfad, Treffer-Ausschnitt mit Hervorhebung, Zeitstempel (bei Transkripten Sprung
  an die Audiostelle), Öffnen im Finder / Standard-App.
- Filter: Quelle, Dateityp, Zeitraum, Sprache.
- Ziel: Ergebnisse < 150 ms bei 100.000 Chunks.

### F6 Fragen (Assistent)

- Chatbereich: Frage in beliebiger Sprache, Antwort in der Sprache der Frage.
- RAG: Top-k Chunks (Standard 8, Token-Budget passend zum Kontext) → Gemma 4 → Antwort mit
  **nummerierten Quellenangaben**, die zur Datei/Stelle springen.
- Bereich einschränken: „nur diese Notiz“, „nur Ordner X“, „letzte 7 Tage“.
- Während einer Aufnahme: „Frag das laufende Gespräch“ (Transkript als Kontext).
- Antworten werden gestreamt; Abbrechen jederzeit.
- Chatverläufe optional als Notiz speichern.

### F7 Einstellungen

- Allgemein: Notizordner, Tastenkürzel, Start bei Anmeldung, Menüleisten-Icon.
- Aufnahme: Geräte, Apps für System-Audio, Rohaudio behalten/Aufbewahrungsdauer, Erinnerungston.
- Sprache: „Meine Sprachen“, chinesische Schrift (vereinfacht/traditionell), Sprache der Zusammenfassungen.
- Modelle: installierte Modelle, Größe, wechseln, löschen; „Modelle nach N Minuten Leerlauf entladen“.
- Quellen: siehe F4.
- Daten: Index neu aufbauen, alle Daten löschen (Notizdateien bleiben, außer explizit gewählt).

### F8 Menüleiste

- Icon mit Aufnahmestatus (rot bei Aufnahme), Menü: Aufnahme starten/stoppen, letzte Notizen,
  Schnellsuche, Fenster öffnen, Beenden.
- Die App kann ohne Fenster im Hintergrund laufen (Indexierung, Aufnahme).

## 6. Bedienoberfläche

```
┌──────────────┬──────────────────────────────────────────────────────┐
│ 🔍 Suchen ⌘K │  Teamrunde Budget           ● 00:23:41  zh · de      │
│              │──────────────────────────────────────────────────────│
│ ◉ Aufnahme   │  Meine Notizen            │  Transkript               │
│              │  - Budget Q1 klären       │  [00:22:58] Ich (de):     │
│ Notizen      │  - Termin mit Li          │   Können wir das bis …    │
│  Heute       │                           │  [00:23:10] Andere (zh):  │
│  Gestern     │  ✨ Ergänzt:              │   我们下周再讨论预算。     │
│  Diese Woche │  - Budget wird nächste    │                           │
│              │    Woche besprochen       │  ▁▃▅▂▁▆▃  ich             │
│ Fragen       │                           │  ▂▁▄▇▅▂▁  andere          │
│ Quellen (3)  │                           │                           │
│ ──────────── │──────────────────────────────────────────────────────│
│ ⬇ Modelle 64%│  [Pause] [Stopp]                     [Frag das Gespräch] │
└──────────────┴──────────────────────────────────────────────────────┘
```

Gestaltung:
- Systemschrift (SF Pro, für Chinesisch PingFang automatisch über Fallback), keine Fremd-Fonts nötig.
- Hell/Dunkel nach System, Akzentfarbe nach System.
- Ruhige Oberfläche, wenige Farben; Sprachkürzel als dezente Badges; Kanäle farblich unterscheidbar.
- Animationen nur dezent (Pegel, Fortschritt, neue Transkriptzeilen einblenden).
- Tastaturbedienbar: `⌘K` Suche, `⌘N` neue Notiz, `⌥⌘R` Aufnahme, `⌘J` Fragen, `⌘,` Einstellungen.
- Lange Listen (Transkript, Notizen, Suchtreffer) virtualisiert.

## 7. Architektur

### 7.1 Komponenten

Ein Prozess, mehrere Worker-Threads. Kein separater Helper-Prozess in v1.

```
                 ┌────────────────── ln-app (GPUI) ──────────────────┐
                 │  Views · Menüleiste (objc2-app-kit) · Hotkeys      │
                 └──────────────▲────────────────────┬────────────────┘
                     Events/State│                    │Commands
                 ┌──────────────┴────────────────────▼────────────────┐
                 │                    ln-core                          │
                 │  Fassade, Jobs, Zustand, Einstellungen              │
                 └──┬─────────┬──────────┬──────────┬──────────┬──────┘
                    │         │          │          │          │
               ln-audio   ln-asr     ln-ingest   ln-index    ln-llm
               Process    VAD +      Parser,     SQLite +    Gemma 4
               Tap, Mic,  Whisper,   OCR,        FTS5 +      (Chat,
               AEC, AAC   LangID,    Chunking,   sqlite-vec  Summary),
                          OpenCC     FSEvents    RRF         Embedding
                    │         │          │          │          │
                    └─────────┴────── ln-models ────┴──────────┘
                              Download, Prüfsummen, Laden/Entladen,
                              RAM-Budget (nur ein großes Modell zugleich)
                                      ln-notes
                              Markdown lesen/schreiben, Journal
```

### 7.2 Cargo-Workspace

| Crate | Aufgabe | Wichtige Abhängigkeiten |
|---|---|---|
| `ln-app` | UI, Fenster, Menüleiste, Hotkeys | `gpui`, `gpui-component`, `objc2-app-kit`, `global-hotkey` |
| `ln-core` | Fassade, Job-Scheduler, Zustand, Einstellungen | `tokio` (rt), `crossbeam-channel`, `serde`, `toml` |
| `ln-audio` | Process Tap, Mic mit Voice Processing, Ringpuffer, AAC-Ausgabe | `objc2-core-audio`, `objc2-av-foundation`, `objc2-audio-toolbox`, `rubato`, `ringbuf` |
| `ln-asr` | VAD, Sprachwahl, Transkription, Nachbearbeitung | `whisper-rs` (whisper.cpp, Metal), Silero VAD, `opencc-rust` o. ä. |
| `ln-llm` | Textgenerierung und Embeddings | `llama-cpp-2` (llama.cpp, Metal) |
| `ln-ingest` | Datei-Erkennung, Parser, OCR, Chunking, Watcher | `notify` (FSEvents), `ignore`, `blake3`, `calamine`, `quick-xml`, `zip`, `objc2-pdf-kit`, `objc2-vision` |
| `ln-index` | Schema, Migrationen, Suche | `rusqlite` (bundled), `sqlite-vec` |
| `ln-notes` | Notizformat, Journal, Wiederherstellung | `pulldown-cmark`, `serde_yaml` |
| `ln-models` | Katalog, Download (Resume), SHA-256, Lebenszyklus | `reqwest` (rustls), `sha2` |
| `ln-cli` | Kommandozeile + MCP-Server (siehe §12) | `clap` |

Regeln:
- Nur `ln-app` kennt GPUI. Alles andere ist ohne UI testbar.
- Laufzeiten (whisper.cpp, llama.cpp) sind hinter Traits (`Transcriber`, `Generator`, `Embedder`)
  gekapselt, damit z. B. LiteRT-LM oder MLX später austauschbar sind.

### 7.3 Threads und Prioritäten

| Thread | QoS | Aufgabe |
|---|---|---|
| UI | user-interactive | GPUI-Hauptthread |
| Audio-Callbacks | real-time | nur in lock-freien Ringpuffer schreiben, keine Allokation |
| Audio-Worker | user-initiated | Resampling, VAD, AAC-Encoding, Journal |
| ASR | user-initiated | ein Thread, Abschnitte nacheinander (beide Kanäle) |
| LLM | user-initiated | eine Anfrage zugleich, streamend |
| Indexer | utility | Parsing, OCR, Embedding im Batch |

### 7.4 Speicherverwaltung der Modelle

Auf 8 GB darf nie alles gleichzeitig geladen sein. `ln-models` führt ein RAM-Budget:

- Budget = min(50 % des physischen RAM, frei verfügbarer Speicher − 1 GB).
- Embedding-Modell (klein) darf neben ASR geladen sein.
- LLM wird geladen, wenn gebraucht; während einer Aufnahme auf 8-GB-Geräten nur, wenn der Nutzer
  explizit fragt (dann ASR-Warteschlange kurz pausieren).
- Entladen nach Leerlauf (Standard 5 min), sofort bei Speicherdruck (`DISPATCH_SOURCE_TYPE_MEMORYPRESSURE`).
- Modelle per `mmap` laden (llama.cpp/whisper.cpp Standard) → schnelles Neu-Laden aus dem Page-Cache.

## 8. Daten

### 8.1 Notizdatei

Ort: `~/Documents/Live Notes/2026/10/2026-10-21 1430 Teamrunde Budget.md` (Ordner einstellbar).
Audio daneben: `…/2026-10-21 1430 Teamrunde Budget.m4a`.

```markdown
---
id: 01JB8Z4K7Q2W3E4R5T6Y7U8I9O     # ULID
title: Teamrunde Budget
created: 2026-10-21T14:30:12+02:00
duration: 00:41:07
languages: [zh, de]
audio: 2026-10-21 1430 Teamrunde Budget.m4a
speakers: { andere: "Frau Li" }
labels: [budget]
app: live-notes/0.1
---

## Zusammenfassung
…

## Aufgaben
- [ ] Budgetentwurf schicken — Ich, bis Freitag

## Meine Notizen
- Budget Q1 klären

## Transkript
[00:00:03] Ich (de): Hallo zusammen, …
[00:00:09] Frau Li (zh): 大家好，…
```

- Die Datei ist die Quelle der Wahrheit. Die App liest sie wieder ein, wenn sie extern bearbeitet wird.
- Zeitstempel- und Sprachinfo stehen pro Zeile im Text, damit die Datei auch ohne App nützlich ist.
- Während der Aufnahme: Journal `…/.livenotes/journal/<id>.jsonl` (ein Segment pro Zeile, fsync
  pro Segment), daraus wird die Markdown-Datei alle 10 s und am Ende geschrieben.

### 8.2 Index (SQLite)

Ort: `~/Library/Application Support/Live Notes/index.sqlite` (WAL-Modus). Vollständig aus den
Dateien wiederherstellbar.

```sql
CREATE TABLE sources (
  id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL, kind TEXT NOT NULL, -- 'notes' | 'folder'
  include_subdirs INTEGER NOT NULL DEFAULT 1, excludes TEXT, paused INTEGER NOT NULL DEFAULT 0,
  added_at INTEGER NOT NULL
);
CREATE TABLE files (
  id INTEGER PRIMARY KEY, source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
  path TEXT UNIQUE NOT NULL, mime TEXT, size INTEGER, mtime INTEGER, hash BLOB,
  title TEXT, lang TEXT, status TEXT NOT NULL,           -- 'pending'|'indexed'|'error'|'skipped'
  error TEXT, indexed_at INTEGER, parser_version INTEGER, embed_model TEXT
);
CREATE TABLE chunks (
  id INTEGER PRIMARY KEY, file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  ord INTEGER NOT NULL, text TEXT NOT NULL, lang TEXT,
  locator TEXT                                            -- JSON: {page:3} | {t_ms:723000} | {line:120}
);
CREATE VIRTUAL TABLE chunks_fts USING fts5(text, content='chunks', content_rowid='id',
  tokenize='unicode61 remove_diacritics 2');
CREATE VIRTUAL TABLE chunks_fts_cjk USING fts5(text, content='chunks', content_rowid='id',
  tokenize='trigram');
CREATE VIRTUAL TABLE chunks_vec USING vec0(embedding int8[256]);   -- rowid = chunks.id
CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY);
```

### 8.3 Chunking

- Struktur-bewusst: Markdown nach Überschriften/Absätzen, PDF nach Seiten/Absätzen, Transkripte nach
  Zeitfenstern (≈ 60 s bzw. Sprecherwechsel), Code nach Funktionen/Leerzeilen.
- Zielgröße 256–512 Tokens, Überlappung 15 %. Jeder Chunk bekommt Dateititel + Überschriftenpfad als
  Präfix für das Embedding (`title: <Titel> | text: <Inhalt>`).
- Sprache pro Chunk über `whatlang` bzw. bei Transkripten aus dem ASR.

### 8.4 Embeddings

- EmbeddingGemma 300M, Prompts laut Modellkarte: Dokumente `title: … | text: …`,
  Anfragen `task: search result | query: …`.
- 768 Dimensionen → **Matryoshka auf 256 gekürzt**, normalisiert, **int8** quantisiert:
  256 Byte pro Chunk (100.000 Chunks ≈ 25 MB).
- Exakte Suche (Brute Force) in `sqlite-vec` reicht bis ~1 Mio. Chunks. Darüber später ANN (HNSW).
- Modellwechsel → `embed_model` in `files` markiert veraltete Einträge, Neuberechnung im Hintergrund.

## 9. Modelle

| Rolle | Modell | Format/Laufzeit | Größe (ca., zu verifizieren) | RAM aktiv |
|---|---|---|---|---|
| ASR | Whisper large-v3-turbo | GGML q5_0, whisper.cpp + Metal | 0,6 GB | ~1 GB |
| ASR (klein, optional) | Whisper small | GGML q5_1 | 0,2 GB | ~0,4 GB |
| VAD | Silero VAD v5 | whisper.cpp-integriert | 2 MB | – |
| Embedding | EmbeddingGemma 300M | GGUF Q8_0, llama.cpp | 0,3 GB | ~0,4 GB |
| LLM | Gemma 4 E2B-it | GGUF Q4_K_M, llama.cpp | ~3 GB | ~3,5 GB |
| LLM | Gemma 4 E4B-it | GGUF Q4_K_M | ~5 GB | ~6 GB |
| LLM | Gemma 4 12B-it | GGUF Q4_K_M | ~7 GB | ~8 GB |
| OCR | Apple Vision | Systemframework | 0 | – |

Auswahl wie bei Foresight nach RAM: < 15 GB → E2B, 15–29 GB → E4B, ≥ 30 GB → 12B.
Sampling-Standard übernommen: T = 0,4, top-k 40, top-p 0,9, Repetition Penalty 1,05, Kontext 4096
(bei E4B/12B 8192, wenn RAM reicht).

Begründung ASR-Wahl: Whisper erkennt ~99 Sprachen, liefert Sprachwahrscheinlichkeiten pro Abschnitt
und transkribiert in der Originalsprache – genau die Lücke von Foresight. Gemma 4 E2B/E4B können
zwar ebenfalls Audio transkribieren (Clips bis 30 s), aber ohne Wort-Zeitstempel und mit schlechterer
Kontrolle über die Sprache; als Alternative im Spike (M0) vergleichen.
Optional später: SenseVoice-Small als besonders schnelles Modell für zh/en/ja/ko/yue.

Download:
- Quellen: Hugging Face (offizielle bzw. `ggml-org`/`ggerganov`-Repos), fester Katalog in der App
  mit URL, Dateigröße und **SHA-256**. Lizenzbedingungen (Gemma Terms of Use) beim Download-Bildschirm
  verlinken.
- Fortsetzbar (HTTP Range), parallel in Teilstücken, Prüfsumme vor Aktivierung.
- Ablage: `~/Library/Application Support/Live Notes/models/`.

## 10. Leistungs- und Speicherbudgets (Referenz M1, 8 GB)

| Messgröße | Ziel |
|---|---|
| Kaltstart bis bedienbares Fenster | < 400 ms |
| RAM im Leerlauf (keine Modelle geladen) | < 80 MB |
| CPU im Leerlauf | < 0,5 % |
| RAM während Aufnahme (ASR + Embedding) | < 1,8 GB |
| Energie während Aufnahme | Akku-Laufzeit-Einbruch ≤ 25 % gegenüber Leerlauf |
| Transkript-Latenz | ≤ 3 s nach Abschnittsende |
| Suche (100.000 Chunks) | < 150 ms |
| Erstes Antwort-Token (E2B, 4k Kontext) | < 2,5 s |
| Indexierung Text/Markdown | > 50 Dateien/s ohne Embedding, > 20 Chunks/s mit |
| App-Größe ohne Modelle | < 40 MB |

Messung automatisiert in einem Benchmark-Ziel (`cargo bench` / eigenes `ln-cli bench`).

## 11. Datenschutz und Sicherheit

- Netzwerkzugriff nur in `ln-models` (Download). Kein anderer Crate darf eine HTTP-Bibliothek
  einbinden (per `cargo deny` geprüft). Einstellung „Offline-Modus“ sperrt auch den Download.
- Keine Telemetrie. Logs nur lokal (`~/Library/Logs/Live Notes/`), ohne Transkriptinhalte.
- Index enthält Textauszüge → liegt im Benutzer-Library-Ordner mit Rechten `0600`. Optional später
  Verschlüsselung (SQLCipher, Schlüssel im Schlüsselbund).
- Prompt-Injection: Inhalte indexierter Dateien sind für das LLM nur Daten; das LLM hat keine Tools,
  die etwas verändern.
- Aufnahme-Zustimmungshinweis (F1), sichtbarer Aufnahmeindikator in Menüleiste.

## 12. Weitere Ideen (Vorschläge)

Für v1 empfohlen:
1. **Notizordner als normale Markdown-Dateien** (bereits spezifiziert) – funktioniert mit Obsidian,
   Git, iCloud Drive; kein Lock-in.
2. **„Meine Sprachen“** statt freier Spracherkennung – deutlich weniger Fehler.
3. **Zwei-Kanal-Sprecherlabels** ohne extra Modell (ich/andere).
4. **Meeting-Erkennung** mit dezenter Aufnahme-Mitteilung.
5. **Sprung vom Transkript ins Audio** (Klick auf Zeitstempel spielt ab).

Für v2:
6. **MCP-Server + CLI** (`ln-cli`): `search`, `get_note`, `ask` als lokale Werkzeuge für
   Claude Code und andere Agenten. Damit wird das lokale Wissen agentisch nutzbar – mit sehr wenig
   Aufwand, weil `ln-core` bereits UI-frei ist.
7. **Sprechertrennung innerhalb eines Kanals** (Diarisierung, z. B. pyannote-Segmentierung als ONNX
   oder Sherpa-ONNX) für mehrere Personen auf der „andere“-Seite.
8. **Live-Assistent** wie Foresight: erkennt Fragen an mich im Gespräch und zeigt passende
   Notizen/Antworten in einem kleinen schwebenden Fenster.
9. **Labels/Sammlungen mit Wiki-Zusammenfassung** und Widerspruchserkennung (wie Foresight).
10. **Übersetzung auf Knopfdruck** pro Abschnitt (Transkript bleibt original, Übersetzung daneben) –
    nützlich für chinesische Gespräche.
11. **Vokabular/Glossar** pro Sprache (Namen, Fachbegriffe) als Whisper-Prompt.
12. **Lokaler Kalender** (EventKit, ohne Google) für Titelvorschläge und Meeting-Vorbereitung.

## 13. Meilensteine

| M | Inhalt | Ergebnis |
|---|---|---|
| M0 Spikes (1–2 Wo.) | GPUI-Spike (10k-Zeilen-Liste, Pinyin-IME, Markdown); Process Tap + Mic-AEC in Rust; Whisper-Sprachwahl auf Testkorpus de/en/zh; Gemma-4-Audio als ASR-Vergleich; sqlite-vec + EmbeddingGemma | Go/No-Go je Technologie, Messwerte |
| M1 Aufnahme & Transkript | ln-audio, ln-asr, ln-notes, minimale UI, Journal/Wiederherstellung | Aufnahme → sprachrichtige Markdown-Notiz |
| M2 Modelle & Start | ln-models, Ein-Bildschirm-Start, Download im Hintergrund | Frischer Mac → App nutzbar in 1 Klick |
| M3 Quellen & Suche | ln-ingest, ln-index, FSEvents, hybride Suche, ⌘K | Ordner hinzufügen → durchsuchbar |
| M4 Assistent | ln-llm, Fragen mit Quellen, Zusammenfassung, Aufgaben, Notiz-Anreicherung | Vollständige v1-Funktion |
| M5 Feinschliff | Menüleiste, Hotkeys, Einstellungen, Performance-Budgets, Signierung/Notarisierung, Updates (Sparkle) | Release-Kandidat |

## 14. Qualitätssicherung

- **ASR-Testkorpus**: je 10 Min. Deutsch, Englisch, Chinesisch (Mandarin), plus gemischte Aufnahmen
  (Sprachwechsel zwischen Sprechern). Kennzahlen: Spracherkennung pro Abschnitt ≥ 97 % korrekt,
  WER (de/en) bzw. CER (zh) nicht schlechter als Whisper-Referenz + 2 Punkte.
- **Retrieval-Testset**: 50 Fragen mit erwarteten Dokumenten, inkl. sprachübergreifender Fragen.
  Kennzahl Recall@10 ≥ 0,85.
- Unit-Tests für Parser, Chunking, RRF, Notizformat (Roundtrip).
- Integrationstests für `ln-core` ohne UI (Audio aus Datei statt Gerät).
- Performance-Budgets (§10) als CI-Benchmark auf Apple-Silicon-Runner.
- `cargo clippy -D warnings`, `cargo deny` (Lizenzen, Netzwerk-Crates), `cargo fmt`.

## 15. Offene Fragen

1. Rohaudio standardmäßig behalten oder nur auf Wunsch? (Entwurf: behalten, 30 Tage)
2. Standard-Notizordner: `~/Documents/Live Notes` oder iCloud Drive?
3. Lizenz des Projekts (Open Source? Gemma-Nutzungsbedingungen beachten)?
4. Soll die App später auch auf Linux/Windows laufen? (Beeinflusst, wie tief wir in AppKit gehen.)
5. Welche Sprachen sind neben Deutsch, Englisch, Chinesisch wichtig (Testkorpus)?
6. Zusammenfassungen standardmäßig automatisch oder nur auf Knopfdruck (Akku/RAM auf 8 GB)?
7. Name der App.
