# Live Notes – Spezifikation v0.1 (Entwurf)

Status: v1.0 umgesetzt (siehe STATUS.md) · Stand 2026-10-21 · Arbeitstitel „Live Notes“

Begleitdokumente:
- [Foresight-Analyse](foresight-analyse.md)
- [ADR-0001: UI-Framework](adr/ADR-0001-ui-framework.md)
- [ADR-0002: Modellanbindung](adr/ADR-0002-modellanbindung.md)

---

## 1. Ziel

Live Notes ist eine native macOS-App in Rust, die

1. **Gespräche mitschneidet** (Mikrofon + System-Audio) und als **sprachrichtiges Transkript**
   in einer Notizdatei speichert,
2. **lokale Verzeichnisse indexiert** und in einer **lokalen** Vektordatenbank durchsuchbar macht,
3. **Fragen beantwortet** über Notizen und Dateien (RAG mit Quellenangaben).

Grundidee: ein **sehr günstiges, multimodales Modell** (Standard: Gemma 4 über OpenRouter) für
Assistent und Bildverständnis, **lokale** Transkription und **lokale** Suche. Jedes Modell ist pro
Aufgabe austauschbar (OpenRouter, LiteLLM-Proxy, Ollama, lokale Modelle) – siehe ADR-0002.
Offline-Betrieb ist möglich, aber kein Muss.

Vorbild ist Google AI Edge Foresight. Unterschiede:

| | Foresight | Live Notes |
|---|---|---|
| UI | Flutter | native Rust-UI (GPUI, Metal) |
| Transkription | faktisch englischzentriert | **in der gesprochenen Sprache**, je Abschnitt erkannt |
| Onboarding | mehrseitiger Wizard, Google-Login | **ein Bildschirm**: Modell-Download bestätigen |
| Modelle | fest Gemma 4 lokal (LiteRT) | **pro Aufgabe wählbar**, Standard Gemma 4 via OpenRouter + lokale ASR/Embeddings |
| Cloud | Google Drive, Docs, Kalender, Firebase | keine Datenquellen-Integration; nur Modell-Endpunkte |
| Speicherort | interne Datenbank | **Markdown-Dateien** in einem sichtbaren Ordner + Index |
| Größe | ~376 MB App + Helper | Ziel < 40 MB App (ohne Modelle) |

## 2. Nicht-Ziele

- Keine Google-, Microsoft- oder sonstige Cloud-Integration (Drive, Docs, Kalender, Login).
- Keine Telemetrie, kein Crash-Upload, keine Remote-Konfiguration.
- Keine Websuche.
- Kein eigenes Backend: Die App spricht direkt mit dem gewählten Modell-Endpunkt.
- Keine Python-Laufzeit (LiteLLM nur als extern betriebener Proxy, siehe ADR-0002).
- Kein Konto, keine Synchronisation zwischen Geräten (Nutzer kann den Notizordner selbst syncen).
- Keine Aktionen nach außen (keine E-Mails, keine Dateiänderungen außerhalb des Notizordners).
- v1 nur macOS auf Apple Silicon. Windows ist vorerst kein Ziel.

## 3. Plattform und Rahmenbedingungen

- **Open Source**, Lizenz **Apache-2.0** (wie Gemma 4, GPUI; kompatibel mit whisper.cpp/llama.cpp (MIT),
  sqlite-vec (MIT/Apache)). Modellgewichte werden nicht im Repo oder in der App mitgeliefert, sondern
  beim ersten Start geladen; Lizenzen siehe §9.4.
- v1: macOS **14.2+** (Core Audio Process Tap), Apple Silicon (M1+). Intel wird nicht unterstützt.
- **v2: Linux** (x86_64 + aarch64, Wayland und X11). Deshalb kapselt `ln-platform` alles
  Betriebssystemspezifische hinter Traits (siehe §7.3). Plattform-APIs werden nur dort verwendet.
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
│  Lokal laden (einmalig):                                  │
│   • Transkription   Whisper large-v3-turbo   ~ 0,6 GB     │
│   • Suche           EmbeddingGemma 300M      ~ 0,3 GB     │
│   • Bilder          Gemma 4 E2B  ~ 3 GB (erst bei Bedarf) │
│   Lizenzen: Apache-2.0, MIT, Gemma Terms [ansehen]        │
│   [x] Ich akzeptiere die Gemma Terms of Use               │
│                                                           │
│  Assistent:  (•) Gemma 4 über OpenRouter  ~0,002 $/Meeting│
│                  OpenRouter-Schlüssel [______________]    │
│                  (auch später in den Einstellungen)       │
│              ( ) Lokal: Gemma 4 E2B, + ~3 GB Download     │
│              ( ) Eigener Endpunkt (LiteLLM, Ollama …)     │
│                                                           │
│                        [ Herunterladen und starten ]      │
└───────────────────────────────────────────────────────────┘
```

- Die Empfehlung hängt vom RAM ab: Bei < 16 GB ist „OpenRouter“ vorausgewählt, ab 16 GB wird
  „Lokal“ als gleichwertige Option angeboten.
- Der Schlüssel ist optional. Ohne Schlüssel funktionieren Aufnahme, Transkription und Suche
  sofort; Assistent-Funktionen zeigen dann einen Hinweis „Schlüssel eintragen“.
- Nach dem Klick landet man **sofort in der App**. Der Download läuft im Hintergrund mit
  Fortschrittsanzeige in der Seitenleiste. Funktionen werden freigeschaltet, sobald ihr Modell da ist
  (Reihenfolge: Embedding → ASR → ggf. lokales LLM, kleinste zuerst).
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
- **Kein Rohaudio auf der Festplatte** (wie Foresight). Audio liegt nur im Arbeitsspeicher: ein
  Ringpuffer pro Kanal hält die letzten 10 Minuten, damit ein Abschnitt während der laufenden Aufnahme
  noch neu transkribiert werden kann (z. B. nach Sprachkorrektur). Mit dem Ende der Aufnahme wird der
  Puffer verworfen. Kein Audio im Journal, in Logs oder in Absturzberichten.
- Zustimmungshinweis vor der ersten Aufnahme (einmalig bestätigen): Alle Teilnehmenden müssen
  einverstanden sein. Optional Erinnerungston alle 30 Minuten.
- Absturzsicherheit: Das **Transkript** wird laufend geschrieben (Segment-Journal, nur Text), nach
  einem Absturz wird die Notiz beim nächsten Start wiederhergestellt.
- Idee: **Meeting-Erkennung** – wenn Zoom/Teams/FaceTime/ein Browser-Call Audio-Ein- und -Ausgabe
  gleichzeitig nutzt, erscheint eine dezente Mitteilung „Aufnahme starten?“ (abschaltbar).

### F2 Sprachrichtige Transkription (Kernunterschied)

Anforderung: Was auf Chinesisch gesagt wird, wird auf Chinesisch transkribiert, Deutsch auf Deutsch usw.
Keine Übersetzung.

Ablauf je Kanal:

1. Resampling auf 16 kHz mono.
2. **VAD** (Silero VAD) schneidet Sprachabschnitte (0,5–25 s, Schnitt bevorzugt in Pausen).
3. **Spracherkennung pro Abschnitt** (eigener Schritt `LanguageDetector`, unabhängig vom
   Transkriptionsmodell): Standard ist die LangID von Whisper. Wir wählen die wahrscheinlichste
   Sprache **innerhalb der vom Nutzer gewählten Sprachen** („Meine Sprachen“). Standard sind die
   **fünf Hauptsprachen Deutsch, Englisch, Chinesisch (Mandarin), Französisch, Spanisch**; weitere
   zuschaltbar. Das verhindert Fehlklassifizierungen (z. B. Niederländisch statt Deutsch).
   Weil der Schritt getrennt ist, können auch Modelle ohne eigene Spracherkennung (z. B. Cohere
   Transcribe) transkribieren: Sie bekommen die erkannte Sprache übergeben.
4. **Transkription in der erkannten Sprache** (bei Whisper `task = transcribe`, bei Cloud-Modellen
   Sprachhinweis-Parameter). Nie übersetzen.
5. Glättung: Ist die Erkennung unsicher (< 0,6) und der Abschnitt kurz (< 2 s), wird die Sprache des
   vorigen Abschnitts desselben Kanals übernommen.
6. Kontext-Prompt (`initial_prompt`) nur aus vorherigen Abschnitten **derselben Sprache**, sonst
   „zieht“ Whisper in die falsche Sprache.
7. Nachbearbeitung je Sprache:
   - Chinesisch: **OpenCC** auf die gewünschte Schrift (vereinfacht/traditionell, Einstellung), da
     Whisper beides mischt. Satzzeichen auf chinesische Vollbreite normalisieren.
   - Alle: Halluzinationsfilter (Wiederholungen, typische Phrasen wie „Untertitel von …“ bei Stille,
     `no_speech_prob`-Schwelle).
8. Manuelle Korrektur: Sprache eines Abschnitts per Klick ändern → Abschnitt wird neu transkribiert,
   solange sein Audio noch im Ringpuffer ist (letzte 10 Minuten der laufenden Aufnahme); danach nur
   noch Text bearbeiten. Pro Aufnahme kann eine Sprache fest vorgegeben werden („Ganze Aufnahme ist
   Chinesisch“).

Darstellung: Jede Zeile trägt Zeitstempel, Kanal und Sprachkürzel, z. B.
`[00:12:03] Andere (zh): 我们下周再讨论预算。`

**Cloud-Transkription mit OpenAI (optional, per API-Schlüssel aktivierbar):**
- Einstellungen → Transkription → „OpenAI-API-Schlüssel“ eintragen → „Verbindung testen“. Erst dann
  erscheinen die OpenAI-Modi; der Schlüssel liegt im Schlüsselbund. Getrennt vom OpenRouter-Schlüssel.
- Drei Modi (global, pro Aufnahme umschaltbar im Aufnahme-Menü):

| Modus | Modell | Verzögerung | Kosten/Std. (Schätzung §9.5) |
|---|---|---|---|
| Lokal (Standard) | Whisper large-v3-turbo | ≤ 3 s | 0 $ |
| OpenAI günstig | `gpt-transcribe`, VAD-Abschnitte einzeln per REST | 2–5 s | ~0,23–0,54 $ |
| OpenAI live | `gpt-live-transcribe`, Realtime-API (WebSocket), Teilergebnisse während des Sprechens | < 1 s | ~0,85–2,04 $ |

- Sprachhinweis: Wir senden „Meine Sprachen“ als `languages`; die von OpenAI erkannte Sprache wird
  pro Abschnitt übernommen (statt der lokalen LangID).
- Beide Kanäle werden getrennt gesendet, damit die Sprecherzuordnung ich/andere erhalten bleibt.
- Die lokale VAD läuft auch im Cloud-Modus (spart Kosten: Stille wird nicht gesendet).
- Fällt die Verbindung aus, schaltet die App automatisch auf lokale Transkription um und meldet das.

Latenz-Ziel: Text erscheint ≤ 3 s nach Ende eines Abschnitts (M1, large-v3-turbo, Metal; Cloud ≤ 1,5 s).
Bei Rückstau: Warteschlange, nichts geht verloren; nach Aufnahmeende wird fertig transkribiert.

Mehrsprachigkeit innerhalb eines Satzes (Code-Switching, z. B. deutsche Sätze mit englischen
Fachbegriffen) bleibt in der Hauptsprache des Abschnitts – das macht Whisper ordentlich.

### F3 Notizen

- Jede Aufnahme erzeugt eine **Markdown-Datei** (Format siehe §8.1). Auch leere Notizen ohne Aufnahme
  sind möglich (eigene Stichpunkte).
- Ansicht während der Aufnahme: eigene Notizen oben (editierbar), Live-Transkript darunter (§6.2).
- Nach jeder Aufnahme **automatisch** (Standard an, abschaltbar), mit dem Assistent-Modell:
  - **Titel** (in der Hauptsprache des Gesprächs),
  - **Zusammenfassung**, **Entscheidungen**, **Aufgaben** (mit Zuständigen, falls Namen fallen;
    „ich“/„andere“ werden nie als Namen verwendet – wie bei Foresight),
  - **Notizen anreichern**: eigene Stichpunkte werden mit Inhalten aus dem Transkript ergänzt.
    Ergänzungen erscheinen eingerückt mit dem Hinweis „Vorschlag“ in Sekundärschrift und lassen sich
    einzeln übernehmen oder verwerfen. Erst übernommene Ergänzungen landen in der Datei.
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
| `.png`, `.jpg`, `.heic` | OCR (macOS: Apple Vision; Linux: Gemma) + **Bildbeschreibung mit dem Bildverständnis-Modell** (Standard: lokales Gemma 4, wahlweise Cloud) |
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
  an die Textstelle), Öffnen im Finder / Standard-App.
- Filter: Quelle, Dateityp, Zeitraum, Sprache.
- Ziel: Ergebnisse < 150 ms bei 100.000 Chunks.

### F6 Fragen (Assistent)

- Einstieg über das Suchfeld (`⌘K`): Frage eingeben, `⌘↩` bzw. Fragezeichen am Ende → Antwort
  erscheint über den Suchtreffern; Folgefragen im selben Feld. Kein separater Chat-Bereich.
- Frage in beliebiger Sprache, Antwort in der Sprache der Frage.
- RAG: Top-k Chunks (Standard 8, Token-Budget passend zum Kontext) → Gemma 4 → Antwort mit
  **nummerierten Quellenangaben**, die zur Datei/Stelle springen.
- Bereich einschränken: „nur diese Notiz“, „nur Ordner X“, „letzte 7 Tage“.
- Während einer Aufnahme: „Frag das laufende Gespräch“ (Transkript als Kontext).
- Antworten werden gestreamt; Abbrechen jederzeit.
- Chatverläufe optional als Notiz speichern.

### F7 Einstellungen

- Allgemein: Notizordner, Tastenkürzel, Start bei Anmeldung, Menüleisten-Icon.
- Aufnahme: Geräte, Apps für System-Audio, Erinnerungston.
- Transkription: Modus (Lokal / OpenAI günstig / OpenAI live), OpenAI-API-Schlüssel, lokales Modell.
- Live-Vorschläge: an/aus, Häufigkeit (zurückhaltend / normal / aktiv), nur lokale Quellen oder auch
  Antworten vom Assistent-Modell, Anzeige im Fenster oder als schwebendes Panel.
- Sprache: „Meine Sprachen“, chinesische Schrift (vereinfacht/traditionell), Sprache der Zusammenfassungen.
- Modelle: pro Rolle (Transkription, Assistent, Bildverständnis, Embeddings) einen **Anbieter** und
  ein **Modell** wählen. Anbieter = Name, Basis-URL, API-Schlüssel (im Schlüsselbund), optional
  Zusatz-Header. Vorkonfiguriert: OpenRouter, Lokal (llama-server), Ollama, LM Studio.
  Modellliste wird vom Endpunkt geladen (`/models`), filterbar nach Eingabe-Modalität und Preis.
  „Verbindung testen“. Installierte lokale Modelle: Größe, löschen; „nach N Minuten Leerlauf entladen“.
- Datenschutz: pro Rolle „darf Daten an Cloud senden“; bei OpenRouter Option „nur Anbieter ohne
  Datenspeicherung“ (`provider.data_collection = "deny"`).
- Quellen: siehe F4.
- Daten: Index neu aufbauen, alle Daten löschen (Notizdateien bleiben, außer explizit gewählt).

### F8 Live-Vorschläge während des Gesprächs (wie Foresight „Live AI Assistant“)

Ziel: Während einer Aufnahme zeigt die App von selbst passende Inhalte aus Notizen und Quellen –
ohne dass man tippen muss.

Ablauf:
1. **Auslöser** (lokal, ohne Kosten): Ein Abschnitt des Kanals „andere“ endet mit einer Frage
   (Fragezeichen, Frage-Wortstellung, Fragepartikel wie zh „吗/呢“), oder es sind seit dem letzten
   Vorschlag ≥ 45 s neue Sprache und ein neues Thema (Embedding-Abstand zum letzten Fenster) da.
   Höchstens ein Vorschlag pro 20 s.
2. **Anfrage bilden**: Das Assistent-Modell bekommt die letzten ~90 s Transkript und gibt zurück:
   `{ "frage_an_mich": bool, "suchanfrage": "…", "sprache": "de" }` (JSON). Bei „nichts Relevantes“
   wird abgebrochen.
   **Ohne verfügbares Assistent-Modell** (kein Schlüssel, offline, oder lokales LLM auf einem Gerät
   < 16 GB RAM während einer Aufnahme): Der letzte Abschnitt (bzw. die erkannte Frage) wird direkt als
   Suchanfrage verwendet; Schritt 4 zeigt dann nur Treffer, keine Kurzantwort.
3. **Suchen**: hybride Suche (F5) lokal, Top-6 Chunks; Notizen der laufenden Aufnahme ausgeschlossen.
4. **Anzeigen**: Karte mit Treffer-Ausschnitten und Quellen sofort (ohne LLM). Wenn eingeschaltet,
   darunter eine **kurze Antwort** (≤ 3 Sätze, Sprache des Gesprächs) mit nummerierten Quellen,
   gestreamt.
5. Karten lassen sich anheften (landen in „Meine Notizen“ unter „Während des Gesprächs“), verwerfen
   oder öffnen. Verworfene Themen werden in dieser Aufnahme nicht erneut vorgeschlagen.

Darstellung: rechte Spalte im Aufnahmefenster; optional ein kleines schwebendes Panel über anderen
Fenstern (für Videocalls im Vollbild), dezent und ohne Ton.

Datenschutz: Mit lokalem Assistent-Modell bleibt alles lokal. Mit Cloud-Modell gehen nur die letzten
~90 s Transkript und die gefundenen Ausschnitte an den Anbieter; Quellen mit „nur lokal“ werden nie
als Kontext gesendet.

Qualitätsziel: ≥ 70 % der angezeigten Karten werden in Tests als „hilfreich“ bewertet; < 1 Karte pro
Minute im Modus „normal“.

### F9 Menüleiste

- Icon mit Aufnahmestatus (rot bei Aufnahme), Menü: Aufnahme starten/stoppen, letzte Notizen,
  Schnellsuche, Fenster öffnen, Beenden.
- Die App kann ohne Fenster im Hintergrund laufen (Indexierung, Aufnahme).

### F10 Markdown-Notizen: erstellen, bearbeiten, anzeigen

**Erstellen und Bearbeiten**
- `⌘N` legt eine neue manuelle Notiz im Notizordner an (Dateiname aus der ersten Überschrift, sonst
  Datum + Uhrzeit; umbenennen jederzeit). Unterordner im Notizordner werden in der Notizliste als
  Ordner gezeigt und können angelegt, umbenannt und per Drag & Drop befüllt werden.
- Bearbeitbar sind alle `.md`-Dateien im Notizordner und in Quellordnern (Öffnen aus Suche, Liste oder
  Finder per „Öffnen mit“). Pro Quelle abschaltbar („nur lesen“).
- **Zwei Modi pro Notiz, ein Tastendruck dazwischen (`⌘E`):**
  - *Lesen* (Standard beim Öffnen): gerenderte Darstellung.
  - *Bearbeiten*: Markdown-Quelltext mit Syntaxhervorhebung (tree-sitter-markdown), Cursor springt an
    die Stelle, die zuletzt im Lesemodus sichtbar war bzw. angeklickt wurde.
  - Bewusst kein WYSIWYG in v1: Ein sauberer Quelltext-Editor plus gute Leseansicht ist robuster und
    verändert die Datei nie unerwartet. Inline-Live-Vorschau (wie Obsidian) ist eine v2-Option.
- Editor-Funktionen: Rückgängig/Wiederholen, Suchen/Ersetzen in der Notiz (`⌘F`), Listen automatisch
  fortsetzen, `Tab`/`⇧Tab` für Einrückung, Formatkürzel (`⌘B`, `⌘I`, `⌘K` im Editor = Link),
  Bilder per Drag & Drop (Kopie in `assets/` neben der Notiz, relativer Link), IME (Pinyin) voll
  unterstützt.
- **Speichern automatisch** (500 ms nach der letzten Änderung, atomar über Temp-Datei + Umbenennen).
  Kein Speichern-Knopf.
- Externe Änderungen (anderer Editor, Sync) werden per Dateiüberwachung erkannt: ohne lokale
  Änderungen still neu laden; mit lokalen Änderungen Hinweis „Datei wurde außerhalb geändert“ mit
  „Meine Version behalten“ / „Andere übernehmen“ / „Vergleichen“.
- Aufnahme-Notizen haben dasselbe Format; die Abschnitte „Meine Notizen“, „Zusammenfassung“ und
  „Aufgaben“ sind frei bearbeitbar, der Abschnitt „Transkript“ ebenfalls (z. B. Tippfehler), wird aber
  während einer laufenden Aufnahme nur von der App geschrieben.

**Darstellung (Lesemodus)**
- CommonMark + GFM: Überschriften, Listen, verschachtelte Listen, **Aufgabenlisten (anklickbar,
  schreibt `[x]` zurück in die Datei)**, Tabellen, Zitate, Code-Blöcke mit Syntaxhervorhebung
  (tree-sitter), Inline-Code, Links, Bilder (lokal, relativ zum Notizpfad), Fußnoten, horizontale Linie.
- Front-Matter wird nicht als Text gezeigt, sondern als eine schlichte Kopfzeile (Datum, Dauer,
  Sprachen, Labels).
- Transkriptzeilen (`[00:12:03] Andere (zh): …`) werden als Zeilen mit Zeitstempel in Sekundärfarbe,
  Sprecher und Sprachkürzel dargestellt – lesbar, ohne die Datei zu verändern.
- Typografie für Lesbarkeit: Textbreite max. ~72 Zeichen (CJK ~40 Zeichen), Zeilenhöhe 1,6,
  Systemschrift, für Code eine Monospace-Systemschrift; Überschriften nur über Größe und Gewicht
  abgesetzt, keine Farbflächen.
- Links: interne Links auf andere `.md` öffnen in der App, externe im Browser.
- Große Dateien (> 1 MB) werden virtualisiert gerendert; Ziel: 10.000-Zeilen-Notiz öffnet in < 150 ms.

### F11 Kategorien (automatisch eingeordnet)

- Der Nutzer legt in den Einstellungen Kategorien an, z. B. „Chinesisch-Kurs“, „Arztagent“
  (Hinweis: „Ideen für neue Anforderungen“), „KI-Projekte“, „Französisch lernen“. Jede Kategorie
  bekommt eine Farbe der macOS-Tags (Rot, Orange, Gelb, Grün, Blau, Lila, Grau); ein Klick auf den
  Farbpunkt wechselt sie.
- Eingeordnet wird von einem **Entscheidungsmodell** (gibt Wahrscheinlichkeiten für feste Antworten
  statt Text): **ein Aufruf pro Notiz** mit einer Ja/Nein-Frage je Kategorie; zugeordnet wird ab
  Wahrscheinlichkeit 0,5, auch mehrere Kategorien.
  - OpenRouter: `POST /api/alpha/decisions`, Fragetyp `noul`; Standardmodell `typesafe/jev-1.14`.
  - OpenAI: `POST /v1/decisions`, Fragetyp `predicate`; Modell `gpt-6-luna`.
  - „Automatisch“ (Standard) nimmt OpenRouter, wenn dessen Schlüssel hinterlegt ist, sonst OpenAI. Das
    ChatGPT-Abo deckt die Decisions-API nicht ab.
- Wann: beim Verlassen des Bearbeitens (⌘E oder Wechsel zu einer anderen Notiz), wenn sich der Text
  geändert hat; nach jeder Aufnahme (nach der Zusammenfassung). Unveränderter Inhalt wird nicht erneut
  geschickt. Hat der Nutzer die Kategorien beim Bearbeiten selbst geändert, bleiben sie. Ein Knopf ordnet
  bestehende Notizen ohne Kategorie nach.
- Gespeichert im Front-Matter der Notiz (`categories:`); eine schlichte Markdown-Notiz bekommt erst dann
  ein Front-Matter, wenn es eine Kategorie gibt. Von Hand eingetragene Kategorien außerhalb der Liste
  bleiben erhalten.
- Anzeige: überlappende Farbpunkte in der Notizliste (wie Finder-Tags), Farbpunkt + Name in der
  Kopfzeile der Notiz.
- **Umbenennen von Hand:** Knopf „Umbenennen“ in der Kopfzeile oder Doppelklick in der Notizliste.
  Ändert den Namen in der Liste (Titel im Front-Matter bzw. erste Überschrift) und den Dateinamen;
  Aufnahmen behalten ihr Datum im Dateinamen und werden danach nie mehr automatisch umbenannt.

## 6. Bedienoberfläche

### 6.1 Gestaltungsgrundsätze

Leitsatz: **Jedes Element hat einen klaren Zweck.** Ein Zweck ist entweder eine Aktion, eine
Information, die man gerade braucht, oder Führung (der Nutzer weiß, was als Nächstes zu tun ist).
Was keinen dieser Zwecke erfüllt, kommt nicht in die Oberfläche.

Regeln:
1. **Inhalt vor Rahmen.** Notizen und Transkripte nehmen den Platz ein, nicht Kopfzeilen, Karten oder
   Dashboards. Keine Startseite mit Kacheln oder Statistiken.
2. **Kein „AI-Slop“:** keine Funkel-Icons (✨), keine Verläufe, keine Glas-Effekte, keine Emoji,
   keine Begrüßungstexte („Wie kann ich helfen?“), keine Maskottchen, keine Füllsätze („Großartig!“).
   KI-erzeugte Inhalte werden schlicht als „Vorschlag“ bzw. „Automatisch erstellt“ in
   Sekundärschrift gekennzeichnet – das hat Zweck (Vertrauen, Unterscheidung), Dekoration nicht.
3. **Eine Hauptaktion pro Ansicht.** Weitere Aktionen sind sekundär oder im Kontextmenü. Keine
   Knopfreihen.
4. **Systemnah.** Systemschrift (SF Pro / PingFang über Fallback, Linux: Systemschrift), Systemfarben,
   Hell/Dunkel nach System, native Menüs und Tastenkürzel. Eine Akzentfarbe; weitere Farben nur mit
   Bedeutung (Aufnahme = Rot, die zwei Sprecherkanäle unterscheidbar).
5. **Zustand nur, wenn er handlungsrelevant ist.** Beispiele mit Zweck: Aufnahme läuft + Dauer,
   Pegel je Kanal (zeigt, ob Ton ankommt), Wolken-Symbol bei Cloud-Nutzung, Download-Fortschritt.
   Ohne Zweck und daher weg: Token-Zähler, Modellnamen in der Hauptansicht, Kosten in der Seitenleiste
   (Kosten stehen in den Einstellungen und pro Notiz in den Details).
6. **Führung statt Erklärung.** Leere Zustände zeigen genau eine nächste Aktion mit einem Satz
   („Noch keine Notizen. Starte eine Aufnahme oder lege mit ⌘N eine Notiz an.“). Keine Touren,
   keine Tooltips-Kaskaden. Berechtigungen und Schlüssel werden genau dann erklärt, wenn sie
   gebraucht werden.
7. **Ruhige Bewegung.** Animation nur für Zustandswechsel (Panel öffnen, neue Transkriptzeile),
   ≤ 150 ms, respektiert „Bewegung reduzieren“.
8. **Tastatur zuerst.** Alles ohne Maus erreichbar: `⌘K` Suchen/Fragen, `⌘N` neue Notiz,
   `⌥⌘R` Aufnahme, `⌘E` Lesen/Bearbeiten, `⌘\` Seitenleiste, `⌘,` Einstellungen.
9. **Text ist UI.** Kurze, konkrete Beschriftungen in der Sprache des Systems, Verben für Aktionen
   („Aufnahme starten“), keine Fachbegriffe wie „RAG“, „Embedding“ oder „Chunk“ in der Oberfläche.

Prüffrage im Review jeder Ansicht: *„Was passiert, wenn wir dieses Element entfernen?“* Lautet die
Antwort „nichts“, wird es entfernt.

### 6.2 Aufbau

Drei Bereiche, nicht mehr:

```
┌───────────────┬──────────────────────────────────────┬───────────────────┐
│ Suchen ⌘K     │ Teamrunde Budget     ● 23:41  de zh  │ Passend dazu      │
│               │──────────────────────────────────────│                   │
│ ● Aufnahme    │ Meine Notizen                        │ Angebot Li, März  │
│               │ - Budget Q1 klären                   │ „… Preis gilt bis │
│ Notizen       │ - Termin mit Li                      │  Ende Q1 …“       │
│  Heute        │                                      │ Verträge/Li.pdf   │
│   Teamrunde   │ Transkript                           │                   │
│  Diese Woche  │ 22:58 Ich (de)                       │ [Anheften]        │
│   1:1 Anna    │   Können wir das bis Freitag …       │                   │
│  Projekte/    │ 23:10 Andere (zh)                    │                   │
│               │   我们下周再讨论预算。                │                   │
│ Quellen       │                                      │                   │
│  Projekte     │──────────────────────────────────────│                   │
│  Verträge     │ ich ▁▃▅▂  andere ▂▄▇▅    [Beenden]   │                   │
└───────────────┴──────────────────────────────────────┴───────────────────┘
```

- **Seitenleiste:** Suchen, Aufnahme-Knopf, Notizen (nach Zeit gruppiert + Ordner), Quellen.
  Fragen an das Wissen laufen über dasselbe Suchfeld (`⌘K`, Eingabe mit Fragezeichen oder `⌘↩` =
  „Frage stellen“) – kein eigener Chat-Bereich in der Navigation.
- **Hauptbereich:** die geöffnete Notiz (Lesen/Bearbeiten). Während einer Aufnahme: eigene Notizen
  oben, Transkript darunter, Aufnahmeleiste unten mit genau einer Hauptaktion („Beenden“);
  Pause im Kontextmenü bzw. `Leertaste` mit Fokus auf der Leiste.
- **Rechte Spalte „Passend dazu“:** nur während einer Aufnahme (Live-Vorschläge, F8) oder auf Wunsch
  bei einer Notiz (verwandte Notizen/Dateien). Sonst ausgeblendet.
- Einstellungen in einem eigenen Fenster (`⌘,`), nicht in der Seitenleiste.
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
               ln-audio   ln-asr     ln-ingest   ln-index    ln-ai
               Process    VAD +      Parser,     SQLite +    Rollen →
               Tap, Mic,  Whisper,   OCR,        FTS5 +      Anbieter;
               AEC,       LangID,    Chunking,   sqlite-vec  OpenAI-kompat.
                          OpenCC     FSEvents    RRF         Client
                    │         │          │          │          │
                    └─────────┴────── ln-models ────┴──────────┤
                              Download, Prüfsummen, Laden/Entladen,   │ HTTPS
                              llama-server starten/stoppen,           ▼
                              RAM-Budget                 OpenRouter · LiteLLM ·
                                      ln-notes           Ollama · 127.0.0.1:llama-server
                              Markdown lesen/schreiben, Journal
```

### 7.2 Cargo-Workspace

| Crate | Aufgabe | Wichtige Abhängigkeiten |
|---|---|---|
| `ln-app` | UI, Fenster, Menüleiste, Hotkeys | `gpui`, `gpui-component`, `objc2-app-kit`, `global-hotkey` |
| `ln-core` | Fassade, Job-Scheduler, Zustand, Einstellungen | `tokio` (rt), `crossbeam-channel`, `serde`, `toml` |
| `ln-platform` | Betriebssystem-Traits und macOS-Umsetzung (§7.3) | `objc2-*`, `keyring`, `notify` |
| `ln-audio` | Aufnahme beider Kanäle (über `ln-platform`), Resampling, 10-Minuten-Ringpuffer im RAM | `rubato`, `ringbuf` |
| `ln-asr` | VAD, Sprachwahl, Transkription, Nachbearbeitung | `whisper-rs` (whisper.cpp, Metal), Silero VAD, `opencc-rust` o. ä. |
| `ln-ai` | Rollen, Anbieter, OpenAI-kompatibler Client (Chat mit Streaming, Bilder, Embeddings, Transkription), Kostenerfassung, Wiederholversuche | `reqwest` (rustls), `eventsource-stream`, `security-framework` (Schlüsselbund) |
| `ln-ingest` | Datei-Erkennung, Parser, OCR, Chunking, Watcher | `notify` (FSEvents), `ignore`, `blake3`, `calamine`, `quick-xml`, `zip`, `objc2-pdf-kit`, `objc2-vision` |
| `ln-index` | Schema, Migrationen, Suche | `rusqlite` (bundled), `sqlite-vec` |
| `ln-notes` | Notizformat, Journal, Wiederherstellung, atomares Speichern, Konflikterkennung, Markdown-Parsing (Front-Matter, Transkriptzeilen, Aufgabenlisten) | `pulldown-cmark` (GFM), `serde_yaml` |
| `ln-app` → Modul `markdown` | Leseansicht (Markdown → GPUI-Elemente), Quelltext-Editor mit Hervorhebung | `gpui-component` (Editor/Input), `tree-sitter`, `tree-sitter-md`, Grammatiken für Code-Hervorhebung |
| `ln-models` | Katalog lokaler Modelle, Download (Resume), SHA-256, Lebenszyklus, `llama-server`-Prozess (mitgeliefert) | `reqwest` (rustls), `sha2` |
| `ln-cli` | Kommandozeile + MCP-Server (siehe §12) | `clap` |

Regeln:
- Nur `ln-app` kennt GPUI. Alles andere ist ohne UI testbar.
- Fachlogik spricht nur Traits an (`LanguageDetector`, `Transcriber`, `StreamingTranscriber`,
  `Generator`, `Embedder`, `ImageDescriber`).
  Umsetzungen: `OpenAiCompatible` (deckt OpenRouter, LiteLLM, Ollama, LM Studio, lokalen
  `llama-server` ab), `OpenAiRealtime` (WebSocket, für `gpt-live-transcribe`) und `LocalWhisper`.
  Weitere (z. B. Qwen3-ASR, MLX) sind möglich, ohne die Fachlogik zu ändern.
- Anbieterspezifisches nur als optionale Zusatzfelder (z. B. OpenRouter-`provider`-Routing,
  `HTTP-Referer`/`X-Title`-Header).
- Fähigkeiten eines Modells (Bild-/Audio-Eingabe, Kontextlänge, Preis) kommen aus `/models`
  des Endpunkts bzw. einer lokalen Liste; die UI bietet für eine Rolle nur passende Modelle an.

### 7.3 Plattform-Abstraktion (Vorbereitung Linux)

`ln-platform` definiert Traits; v1 implementiert macOS, v2 Linux.

| Trait | macOS (v1) | Linux (v2) |
|---|---|---|
| `SystemAudioCapture` | Core Audio Process Tap | PipeWire Monitor-Source (pro App über PipeWire-Graph) |
| `MicCapture` (+ AEC) | Voice-Processing-I/O | PipeWire + `webrtc-audio-processing` |
| `Ocr` | Apple Vision | Gemma-Bildverständnis (Tesseract als Rückfall) |
| `PdfText` | PDFKit | pdfium (`pdfium-render`) |
| `SecretStore` | Schlüsselbund | Secret Service (über `keyring`) |
| `FileWatcher` | FSEvents | inotify (beides über `notify`) |
| `Tray`, `GlobalHotkey` | NSStatusItem, Carbon-Hotkey | `tray-icon` (StatusNotifierItem), `global-hotkey` / XDG-Portal |
| GPU | Metal (GPUI, whisper.cpp, llama.cpp) | Vulkan (GPUI), CUDA/Vulkan/CPU (whisper.cpp, llama.cpp) |

Regel: Kein Crate außer `ln-platform` und `ln-app` importiert `objc2-*`. CI baut ab M3 zusätzlich
`ln-core` und alle Nicht-UI-Crates für Linux, damit keine macOS-Abhängigkeit unbemerkt einsickert.

### 7.4 Threads und Prioritäten

| Thread | QoS | Aufgabe |
|---|---|---|
| UI | user-interactive | GPUI-Hauptthread |
| Audio-Callbacks | real-time | nur in lock-freien Ringpuffer schreiben, keine Allokation |
| Audio-Worker | user-initiated | Resampling, VAD, Journal (nur Text) |
| ASR | user-initiated | ein Thread, Abschnitte nacheinander (beide Kanäle) |
| LLM | user-initiated | eine Anfrage zugleich, streamend |
| Indexer | utility | Parsing, OCR, Embedding im Batch |

### 7.5 Speicherverwaltung der Modelle

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
Die Markdown-Datei ist die **Quelle der Wahrheit** für Notiz und Transkript (entschieden 2026-10-21).
Es gibt keine Audiodatei.

```markdown
---
id: 01JB8Z4K7Q2W3E4R5T6Y7U8I9O     # ULID
title: Teamrunde Budget
created: 2026-10-21T14:30:12+02:00
duration: 00:41:07
languages: [zh, de]
transcription: local/whisper-large-v3-turbo   # oder openai/gpt-live-transcribe
speakers: { andere: "Frau Li" }
labels: [budget]
app: live-notes/0.1
---

<!-- ln:summary -->
## Zusammenfassung
…

<!-- ln:tasks -->
## Aufgaben
- [ ] Budgetentwurf schicken — Ich, bis Freitag

<!-- ln:notes -->
## Meine Notizen
- Budget Q1 klären

<!-- ln:transcript -->
## Transkript
[00:00:03] Ich (de): Hallo zusammen, …
[00:00:09] Frau Li (zh): 大家好，…
```

- **Abschnitte werden über die unsichtbaren Marker `<!-- ln:… -->` erkannt**, nicht über den
  Überschriftentext. Überschriften und Sprecherlabels („Ich“/„Andere“ bzw. „Me“/„Others“) stehen in
  der Sprache der Oberfläche zum Zeitpunkt der Erstellung; intern gelten die festen Kanal-IDs
  `me`/`others`. Die Transkriptzeile hat das feste Muster
  `[HH:MM:SS] <Label> (<iso639-1>): <Text>`; das Label ist entweder das Kanal-Label oder ein Name aus
  `speakers` im Front-Matter.
- Dateiname einer Aufnahme zunächst `JJJJ-MM-TT HHMM Aufnahme.md` (bzw. „Recording“), nach der
  automatischen Zusammenfassung Umbenennung auf den erzeugten Titel (bei Namenskollision Suffix ` 2`).
  Eine manuell gesetzte Benennung wird nie überschrieben.

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
| LLM lokal (optional) | Gemma 4 E2B-it | GGUF Q4_K_M, llama-server | ~3 GB | ~3,5 GB |
| LLM lokal (optional) | Gemma 4 E4B-it | GGUF Q4_K_M | ~5 GB | ~6 GB |
| LLM lokal (optional) | Gemma 4 12B-it | GGUF Q4_K_M | ~7 GB | ~8 GB |
| OCR | Apple Vision | Systemframework | 0 | – |

### 9.1 Standard pro Rolle

| Rolle | Standard | Konfigurierbare Alternative |
|---|---|---|
| Spracherkennung (LangID) | lokal, Whisper | Cloud-Modell meldet Sprache selbst |
| Live-Transkription | lokal, Whisper large-v3-turbo | OpenAI `gpt-live-transcribe` |
| Nach-/Datei-Transkription | lokal, Whisper | OpenAI `gpt-transcribe`, OpenRouter |
| Assistent (Zusammenfassung, Fragen) | Cloud, `google/gemma-4-26b-a4b-it` (OpenRouter) | jedes OpenAI-kompatible Modell, lokal Gemma 4 E2B/E4B |
| Bildverständnis | **lokal, Gemma 4 E2B/E4B** (läuft im Hintergrund, nie während einer Aufnahme auf 8-GB-Geräten) | Cloud, z. B. `google/gemma-4-26b-a4b-it` |
| Embeddings | lokal, EmbeddingGemma 300M | Cloud, z. B. `qwen/qwen3-embedding-4b` |

Hinweis 8-GB-Mac: Weil Bildverständnis standardmäßig lokal ist, wird beim ersten Start auch
Gemma 4 E2B (~3 GB) geladen – aber erst, nachdem ASR und Embedding fertig sind, und nur wenn eine
Quelle Bilder enthält. Bilder werden gesammelt und in Leerlaufphasen am Stück verarbeitet.

### 9.2 Gehostete Modelle

Preise laut OpenRouter-Modell-API am 2026-10-21, pro 1 Mio. Tokens:

| Rolle | Modell-ID | Eingabe | Preis Ein/Aus |
|---|---|---|---|
| Assistent + Bildverständnis | `google/gemma-4-26b-a4b-it` | Text, Bild, Video | 0,09 $ / 0,30 $ |
| Assistent (stärker) | `google/gemma-4-31b-it` | Text, Bild, Video | 0,20 $ / 0,40 $ |
| Transkription (Audio-Chat) | `google/gemini-2.5-flash-lite` | + Audio | 0,10 $ / 0,40 $ |
| Embeddings | `qwen/qwen3-embedding-4b` | Text | 0,02 $ |

OpenAI-Transkription (laut OpenAI-Doku und Changelog, Stand 2026-10-21):

| Modell | Zweck | Seit |
|---|---|---|
| `gpt-live-transcribe` | Live-Streaming über Realtime-API, niedrige Latenz | 28.07.2026 |
| `gpt-transcribe` | Dateien und finale Transkripte; Originalsprache, liefert erkannte Sprachen, `languages`-Hinweise | 28.07.2026 |
| `gpt-realtime-whisper` | Streaming, 0,017 $/min | 07.05.2026 |

Abgekündigt zum 26.02.2027: `whisper-1`, `gpt-4o-transcribe`, `gpt-4o-mini-transcribe`,
`gpt-4o-transcribe-diarize`. Diese unterstützen wir nicht.
Offen: Wort-Zeitstempel bei `gpt-transcribe` sind nicht dokumentiert → Zeitstempel kommen dann aus
unserer VAD (Abschnittsbeginn), was für Notizen reicht. Preise für `gpt-transcribe` und
`gpt-live-transcribe` vor M0 auf der Preisseite prüfen.

### 9.3 Lokale Transkription: Kandidaten

Recherche Stand 2026-10-21 (Hugging Face Open ASR Leaderboard und Berichte; Ranglisten schwanken je
nach Messzeitpunkt):

| Modell | Größe | Sprachen | de/en/zh/fr/es | LangID | Lizenz | Einschätzung |
|---|---|---|---|---|---|---|
| Whisper large-v3-turbo | 0,8B | 99 | ✅ | ✅ | MIT | Breiteste Abdeckung, ausgereifte Laufzeit (whisper.cpp: Metal, CUDA, Vulkan, CPU) → **Standard** |
| Qwen3-ASR 1.7B (Jan. 2026) | 1,7B | 52 (30 + 22 chin. Dialekte) | ✅ | ✅ | Apache-2.0 | Stärkster Kandidat für Chinesisch, hat LangID + Zeitstempel; Laufzeit in Rust noch unreif (GGUF über llama.cpp-mtmd experimentell) → **Benchmark in M0** |
| Cohere Transcribe 03-2026 | 2B | 14 | ✅ | ❌ | Apache-2.0 | War Spitze im Leaderboard (5,42 % WER), aber ohne Spracherkennung und schwach bei Sprachwechseln → nur mit unserer LangID nutzbar, **Benchmark in M0** |
| IBM Granite Speech 4.1 2B | 2B | 6 | prüfen | – | Apache-2.0 | Sehr genau (5,33 % WER), zu wenige Sprachen |
| NVIDIA Parakeet TDT 0.6B v3 | 0,6B | 25 europäische | ❌ zh | ✅ | CC-BY-4.0 | Extrem schnell, aber kein Chinesisch |

„Fähigstes“ Open-Source-Modell für **unsere fünf Sprachen mit Spracherkennung**: Qwen3-ASR 1.7B
(Genauigkeit, besonders Chinesisch) bzw. Whisper large-v3-turbo (Reife, Plattformen). Entscheidung
nach dem M0-Benchmark auf unserem Testkorpus; dank `Transcriber`-Trait ist der Wechsel billig.

### 9.4 Lizenzen der Modelle

| Modell | Lizenz | Folge |
|---|---|---|
| Gemma 4 (E2B, E4B, 26B-A4B, 31B) | Apache-2.0 | Unkritisch, Lizenzhinweis anzeigen |
| EmbeddingGemma 300M | Gemma Terms of Use + Prohibited Use Policy | Beim Download Bedingungen verlinken und bestätigen lassen; Nutzungsbeschränkungen in der App-Lizenzübersicht weitergeben |
| Whisper | MIT | Unkritisch |
| Qwen3-ASR, Cohere Transcribe | Apache-2.0 | Unkritisch |

Die App selbst enthält keine Gewichte; der Download-Bildschirm zeigt pro Modell Lizenz und Link.
Ein Ordner `LICENSES/models/` im Repo dokumentiert die Bedingungen.

### 9.5 Kosten pro Gesprächsstunde (Schätzung)

Preise: OpenAI-Preisseite und OpenRouter-Modell-API, Stand 2026-10-21. Annahmen für 1 Stunde Meeting:
~50 Min. tatsächliche Sprache über beide Kanäle (Rest Stille, die die VAD nicht sendet), ~15.000 Tokens
Transkript, Live-Vorschläge im Modus „normal“ mit ~60 Auslösern/Std.

| Posten | Rechnung | Kosten/Std. |
|---|---|---|
| Transkription lokal (Whisper) | – | **0 $** |
| Transkription „OpenAI günstig“ (`gpt-transcribe`, 0,0045 $/min) | 50 min Sprache · 0,0045 $ | **~0,23 $** (bis 0,54 $, falls beide Kanäle die volle Stunde senden) |
| Transkription „OpenAI live“ (`gpt-live-transcribe`, 0,017 $/min) | 50 min · 0,017 $ | **~0,85 $** (bis 2,04 $, falls nach Sitzungsdauer × 2 Kanäle abgerechnet wird) |
| Zusammenfassung, Aufgaben, Notiz-Anreicherung (Gemma 4 26B, 0,09 $/0,30 $ pro 1 Mio. Tokens) | ~16k Tokens rein, ~1,5k raus | ~0,002 $ |
| Live-Vorschläge: Anfrage bilden | 60 × (~700 rein, ~60 raus) | ~0,005 $ |
| Live-Vorschläge: Kurzantworten | 60 × (~2.800 rein, ~200 raus) | ~0,019 $ |
| Embeddings, Suche | lokal | 0 $ |

Summe pro Stunde:

| Transkriptionsmodus | Gesamt/Std. | 20 Std. Meetings/Monat |
|---|---|---|
| Lokal | **~0,03 $** | ~0,60 $ |
| OpenAI günstig | **~0,26 $** (max. ~0,57 $) | ~5 $ |
| OpenAI live | **~0,88 $** (max. ~2,07 $) | ~18 $ (max. ~41 $) |

Chinesisch erzeugt pro Minute etwas mehr Tokens als Deutsch; an den LLM-Kosten ändert das wenig.
Die App zeigt die tatsächlichen Kosten pro Notiz und pro Monat (aus den `usage`-Angaben der APIs bzw.
gesendeter Audiodauer × Minutenpreis).

### 9.6 Lokale LLMs, Kosten und Download

Wichtig: **Gemma mit Audio-Eingabe (E2B/E4B) gibt es nur lokal**, nicht auf OpenRouter. Die
Gemma-Modelle auf OpenRouter verstehen Bilder und Video, aber kein Audio. Für unseren Aufbau ist das
kein Problem, weil die Transkription ohnehin lokal mit Whisper läuft.

Lokale LLM-Auswahl wie bei Foresight nach RAM: < 15 GB → E2B, 15–29 GB → E4B, ≥ 30 GB → 12B.
Sampling-Standard übernommen: T = 0,4, top-k 40, top-p 0,9, Repetition Penalty 1,05. Kontext lokal
4096 (bei E4B/12B 8192, wenn RAM reicht); gehostet bis 32k für ganze Meeting-Transkripte.

Kostenkontrolle: Die App erfasst Tokens und Kosten pro Anfrage (aus der `usage`-Antwort), zeigt
Monatssumme in den Einstellungen und erlaubt ein Monatslimit, ab dem Cloud-Anfragen nachfragen.

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
- Ablage: macOS `~/Library/Application Support/Live Notes/models/`, Linux
  `$XDG_DATA_HOME/live-notes/models/`.

## 10. Leistungs- und Speicherbudgets (Referenz M1, 8 GB)

| Messgröße | Ziel |
|---|---|
| Kaltstart bis bedienbares Fenster | < 400 ms |
| RAM im Leerlauf (keine Modelle geladen) | < 80 MB |
| CPU im Leerlauf | < 0,5 % |
| RAM während Aufnahme (ASR + Embedding, Assistent gehostet) | < 1,8 GB |
| Erstes Antwort-Token (Gemma 4 26B via OpenRouter) | < 1,5 s |
| Energie während Aufnahme | Akku-Laufzeit-Einbruch ≤ 25 % gegenüber Leerlauf |
| Transkript-Latenz | ≤ 3 s nach Abschnittsende |
| Suche (100.000 Chunks) | < 150 ms |
| Erstes Antwort-Token (E2B, 4k Kontext) | < 2,5 s |
| Indexierung Text/Markdown | > 50 Dateien/s ohne Embedding, > 20 Chunks/s mit |
| App-Größe ohne Modelle | < 40 MB |

Messung automatisiert in einem Benchmark-Ziel (`cargo bench` / eigenes `ln-cli bench`).

## 11. Datenschutz und Sicherheit

- Netzwerkzugriff nur in `ln-models` (Download) und `ln-ai` (Modell-Endpunkte). Kein anderer Crate
  darf eine HTTP-Bibliothek einbinden (per `cargo deny` geprüft). Einstellung „Offline-Modus“ sperrt
  beides; dann laufen nur lokale Rollen.
- **Was die Cloud sieht, ist sichtbar:** Jede Cloud-Anfrage zeigt in der UI ein kleines Wolken-Symbol
  mit Anbieter und Modell. Vor dem ersten Senden an einen neuen Anbieter einmal bestätigen.
- Standard: Transkription und Embeddings lokal → Audio verlässt den Rechner nie, Dateiinhalte nur
  als ausgewählte Chunks für eine Frage bzw. einen Live-Vorschlag, Transkripte nur für eine
  Zusammenfassung. Im OpenAI-Transkriptionsmodus geht Audio (nur Sprachabschnitte) an OpenAI;
  das zeigt die Aufnahmeleiste dauerhaft an.
- Audio wird nie auf die Festplatte geschrieben (§5 F1).
- Pro Notiz/Quelle „nur lokal“ markierbar; solche Inhalte gehen nie an Cloud-Rollen
  (dann lokales LLM oder Funktion gesperrt).
- API-Schlüssel nur im macOS-Schlüsselbund, nie in Konfigurationsdateien oder Logs.
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
5. **Live-Vorschläge** während des Gesprächs – jetzt fester Bestandteil von v1 (§5 F8).

Für v2:
6. **MCP-Server + CLI** (`ln-cli`): `search`, `get_note`, `ask` als lokale Werkzeuge für
   Claude Code und andere Agenten. Damit wird das lokale Wissen agentisch nutzbar – mit sehr wenig
   Aufwand, weil `ln-core` bereits UI-frei ist.
7. **Sprechertrennung innerhalb eines Kanals** (Diarisierung, z. B. pyannote-Segmentierung als ONNX
   oder Sherpa-ONNX) für mehrere Personen auf der „andere“-Seite.
8. **Live-Vorschläge mit lokalem Klassifikator** statt LLM-Aufruf für Schritt 2 von F8
   (kleines Modell erkennt „Frage an mich“) – spart Kosten und Latenz.
9. **Labels/Sammlungen mit Wiki-Zusammenfassung** und Widerspruchserkennung (wie Foresight).
10. **Übersetzung auf Knopfdruck** pro Abschnitt (Transkript bleibt original, Übersetzung daneben) –
    nützlich für chinesische Gespräche.
11. **Vokabular/Glossar** pro Sprache (Namen, Fachbegriffe) als Whisper-Prompt.
12. **Lokaler Kalender** (EventKit, ohne Google) für Titelvorschläge und Meeting-Vorbereitung.

## 13. Meilensteine

| M | Inhalt | Ergebnis |
|---|---|---|
| M0 Spikes (1–2 Wo.) | GPUI-Spike (10k-Zeilen-Liste, Pinyin-IME, Markdown); Process Tap + Mic-AEC in Rust; Whisper-Sprachwahl auf Testkorpus de/en/zh; Gemma 4 26B via OpenRouter vs. lokal E2B für Zusammenfassungen (Qualität/Kosten/Latenz, zh+de); sqlite-vec + EmbeddingGemma | Go/No-Go je Technologie, Messwerte |
| M1 Aufnahme & Transkript | ln-audio, ln-asr, ln-notes, minimale UI, Journal/Wiederherstellung | Aufnahme → sprachrichtige Markdown-Notiz |
| M1b Notizen | Markdown-Leseansicht, Editor (F10), Notizliste mit Ordnern, Autospeichern, Konflikterkennung | Manuelle Notizen vollständig nutzbar |
| M2 Modelle & Start | ln-models, ln-ai (Anbieter, Schlüsselbund), Ein-Bildschirm-Start, Download im Hintergrund | Frischer Mac → App nutzbar in 1 Klick |
| M3 Quellen & Suche | ln-ingest, ln-index, FSEvents, hybride Suche, ⌘K | Ordner hinzufügen → durchsuchbar |
| M4 Assistent | Fragen mit Quellen, Zusammenfassung, Aufgaben, Notiz-Anreicherung, **Live-Vorschläge (F8)**, OpenAI-Transkriptionsmodi, Bildbeschreibung beim Indexieren, Kostenanzeige | Vollständige v1-Funktion |
| M5 Feinschliff | Menüleiste, Hotkeys, Einstellungen, Performance-Budgets, Signierung/Notarisierung, Updates (Sparkle) | Release-Kandidat |

## 14. Qualitätssicherung

- **ASR-Testkorpus**: je 10 Min. Deutsch, Englisch, Chinesisch (Mandarin), Französisch, Spanisch,
  plus gemischte Aufnahmen (Sprachwechsel zwischen Sprechern, z. B. de↔zh, fr↔en). Kennzahlen:
  Spracherkennung pro Abschnitt ≥ 97 % korrekt, WER (de/en/fr/es) bzw. CER (zh) nicht schlechter als
  Whisper-Referenz + 2 Punkte. Derselbe Korpus vergleicht in M0 Whisper, Qwen3-ASR, Cohere
  Transcribe und `gpt-transcribe`.
- **Retrieval-Testset**: 50 Fragen mit erwarteten Dokumenten, inkl. sprachübergreifender Fragen.
  Kennzahl Recall@10 ≥ 0,85.
- Unit-Tests für Parser, Chunking, RRF, Notizformat (Roundtrip).
- Integrationstests für `ln-core` ohne UI (Audio aus Datei statt Gerät).
- Performance-Budgets (§10) als CI-Benchmark auf Apple-Silicon-Runner.
- `cargo clippy -D warnings`, `cargo deny` (Lizenzen, Netzwerk-Crates), `cargo fmt`.

## 15. Offene Fragen

1. Wie genau rechnet OpenAI `gpt-live-transcribe` ab: nach gesendeter Audiozeit oder nach
   Sitzungsdauer? Bestimmt, ob die Kosten bei ~0,85 $ oder ~2,04 $ pro Stunde liegen (§9.5). In M0 messen.

Entschieden am 2026-10-21:
- **Kein Rohaudio** auf der Festplatte, nur 10-Minuten-Ringpuffer im RAM (§5 F1).
- **Transkripte und Notizen als Markdown-Dateien**, Datenbank nur als Index (§8.1).
- **Live-Vorschläge** während des Gesprächs (§5 F8).
- **Markdown-Notizen erstellen, bearbeiten und schön anzeigen** (§5 F10).
- **Schlichte UI ohne Elemente ohne Zweck** (§6.1).
- **OpenAI-Transkription** per eigenem API-Schlüssel aktivierbar, Modi „günstig“ und „live“ (§5 F2).
- Notizordner: `~/Documents/Live Notes`.
- Open Source, Apache-2.0, Modelllizenzen beachten (§3, §9.4).
- Linux später (v2), Windows kein Fokus (§3, §7.3).
- Hauptsprachen: Deutsch, Englisch, Chinesisch, Französisch, Spanisch (§5 F2, §14).
- Zusammenfassung automatisch nach jeder Aufnahme (§5 F3).
- Bildverständnis standardmäßig lokal, Cloud konfigurierbar (§9.1).
- Live-Transkription optional über die Cloud, inkl. OpenAI `gpt-live-transcribe` (§5 F2, §9.2).

## 16. Umsetzungsentscheidungen für den ersten Durchlauf (verbindlich)

Umsetzungsstand und Abweichungen: [STATUS.md](STATUS.md).

Entschieden am 2026-10-21 nach Empfehlung. Bei Widerspruch zu früheren Abschnitten gilt dieser.

**Umfang des ersten Durchlaufs („v1.0“)**
- Enthalten: F1 Aufnahme, F2 Transkription (Modi „Lokal“ und „OpenAI günstig“), F3 Notizen inkl.
  automatischer Zusammenfassung, F4 Quellen (ohne Bildbeschreibung, siehe unten), F5 Suche,
  F6 Fragen, F7 Einstellungen, F8 Live-Vorschläge in der rechten Spalte, F9 Menüleiste,
  F10 Markdown-Editor und -Leseansicht, Erster Start (§4).
- Später (v1.1 ff.): Modus „OpenAI live“ (`gpt-live-transcribe`), schwebendes Vorschlags-Panel,
  lokale Bildbeschreibung mit Gemma 4 (in v1.0 nur Texterkennung in Bildern über Apple Vision),
  Meeting-Erkennung, Auto-Update, `ln-cli`/MCP-Server, Linux.
- Der Download-Bildschirm (§4) bietet in v1.0 deshalb nur Whisper + EmbeddingGemma an; Gemma 4 lokal
  nur, wenn der Assistent auf „Lokal“ gestellt wird.

**Vorgehen**
- Die Spikes aus M0 entfallen als Entscheidungspunkt. Feste Wahl: GPUI + gpui-component, Whisper
  large-v3-turbo über `whisper-rs`, SQLite + `sqlite-vec`, EmbeddingGemma über llama.cpp.
  Der Agent liefert stattdessen ein Benchmark-Ziel (`cargo run -p ln-bench`), mit dem die
  Alternativen (Qwen3-ASR, Cohere Transcribe, `gpt-transcribe`) später verglichen werden.
- Fällt GPUI bei Pinyin-Eingabe oder Markdown-Darstellung durch, dokumentiert der Agent das im
  Repo (`docs/adr/`) und baut trotzdem weiter; ein Wechsel auf Iced ist eine eigene Aufgabe.

**Definition of Done für den Agenten**
- `cargo build --release` und `cargo test --workspace` grün, `cargo clippy -- -D warnings` und
  `cargo deny check` ohne Fehler.
- App startet, erster Start und alle Ansichten sind bedienbar; Screenshots der Hauptansichten
  liegen unter `docs/screenshots/`.
- Aufnahme-Pipeline ist mit Audiodateien statt Live-Geräten getestet (Datei → VAD → LangID →
  Transkript → Markdown), inklusive Sprachwechsel-Testfällen.
- Nicht vom Agenten prüfbar und daher Abnahme durch den Projektinhaber: echte Aufnahme mit Mikrofon
  und System-Audio, Berechtigungsdialoge, Echo-Unterdrückung, subjektive Qualität der Live-Vorschläge.
- Eine Datei `docs/STATUS.md` listet, was umgesetzt, was offen und was nur mit Testdaten geprüft ist.

**Oberfläche**
- Sprachen der Oberfläche: Deutsch und Englisch (Systemsprache, Rückfall Englisch), alle Texte
  über eine Übersetzungsdatei (z. B. Fluent `.ftl`), keine fest verdrahteten Strings.
- Live-Vorschläge ohne Assistent-Modell: nur Treffer (siehe F8).

**Projekt und Infrastruktur**
- App-Name „Live Notes“, Bundle-ID `works.karma.livenotes`, Binärname `live-notes`.
- Repository: GitHub, Name `live-notes`. Anlegen und Veröffentlichen erst nach ausdrücklicher
  Freigabe durch den Projektinhaber; bis dahin nur lokal.
- Signierung: Bis eine Apple Developer ID vorliegt, wird lokal mit einem stabilen selbst erzeugten
  Codesigning-Zertifikat signiert (damit macOS die Berechtigungen nicht nach jedem Build neu abfragt).
  Notarisierung und Verteilung über GitHub Releases folgen, sobald die Developer ID da ist.
- Kein Auto-Update in v1.0; Releases nur über GitHub.
- CI: GitHub Actions auf macOS-ARM-Runnern (Build, Tests, Clippy, `cargo deny`); ab Linux-Vorbereitung
  zusätzlich ein Linux-Job für die UI-freien Crates.

**Tests**
- Cloud-Anbieter werden in Tests durch einen lokalen Nachbau-Server (OpenAI-kompatibel, inkl.
  `/audio/transcriptions` und `/models`) ersetzt. Echte Schlüssel nur für einen optionalen,
  manuell gestarteten Testlauf über die Umgebungsvariablen `LN_TEST_OPENROUTER_KEY` und
  `LN_TEST_OPENAI_KEY`; Kostenobergrenze pro Lauf 0,50 $.
- ASR-Testkorpus: öffentlicher Datensatz **FLEURS** (CC-BY-4.0) für Deutsch, Englisch, Mandarin,
  Französisch, Spanisch. Gespräche mit Sprachwechsel werden daraus per Skript zusammengesetzt
  (`scripts/build-test-corpus`), nicht ins Repo eingecheckt, sondern beim Testlauf geladen und
  zwischengespeichert. Eigene Aufnahmen ergänzt der Projektinhaber später.

**Vom Agenten selbst zu klären (keine Rückfrage nötig)**
- Genaue Crate-Versionen (GPUI auf festen Commit pinnen), Prompts, Modell-URLs und SHA-256,
  Details von Schema und Chunking, Fehlertexte.
- Ob EmbeddingGemma auf Hugging Face eine Anmeldung bzw. Lizenzbestätigung verlangt; falls ja, eine
  frei ladbare GGUF-Fassung mit gleicher Lizenz verwenden oder den Download-Bildschirm um einen
  Hinweis ergänzen.

**Voraussetzungen der Entwicklungsumgebung**
- Rust stable ≥ 1.99 (installiert), Xcode 27.1 mit Metal-Toolchain (für GPUI-Shader und Signierung),
  CMake (installiert), ~30 GB freier Speicher.
