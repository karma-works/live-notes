<p align="center"><img src="docs/logo.svg" width="128" height="128" alt="Live Notes"></p>

# Live Notes

Native Desktop-App in Rust: Gespräche mitschneiden und **in der gesprochenen Sprache** transkribieren,
Markdown-Notizen schreiben, lokale Ordner durchsuchen und Fragen an das eigene Wissen stellen.

Transkription und Suche laufen lokal (Whisper, EmbeddingGemma, SQLite). Der Assistent nutzt ein
günstiges, multimodales Modell (Standard: Gemma 4 über OpenRouter), austauschbar über jede
OpenAI-kompatible API (OpenRouter, LiteLLM-Proxy, Ollama, lokales Gemma). Audio wird nie gespeichert.

Mit **Gedanken ordnen** lassen sich frei gesprochene Ideen über das Mikrofon in ein zusammenhängendes
Dokument überführen, etwa eine Anforderungsspezifikation. Verwandte Aussagen werden gruppiert,
Wiederholungen entfernt und Details sowie offene Widersprüche erhalten. Das Ergebnis enthält keine
Zeitstempel und kein Transkript. Bis zur erfolgreichen Konsolidierung bleibt der Ausgangstext zur
Wiederherstellung gespeichert; bei Fehlern gibt es in der Notiz „Gedanken erneut ordnen“.
Der Modus verwendet das konfigurierte Zusammenfassungsmodell, auch bei deaktivierter automatischer
Meeting-Zusammenfassung.

Whisper wird erst für die Transkription geladen und nach deren Abschluss wieder freigegeben.
Lokale Chat- und Suchmodelle werden nach der eingestellten Leerlaufzeit entladen; laufende
Anfragen sind davon ausgenommen.

![Aufnahme mit Live-Transkript in fünf Sprachen](docs/screenshots/08-recording.png)

Status: v1.0 umgesetzt, Abnahme mit echten Geräten offen – siehe [STATUS](docs/STATUS.md).
Lizenz: [Apache-2.0](LICENSE). Plattform: macOS 14.2+ auf Apple Silicon und Linux-Alpha für PipeWire.

## Schnellstart

```bash
brew install llama.cpp        # llama-server für EmbeddingGemma und lokales Gemma
cargo run --release -p ln-app
```

Beim ersten Start lädt die App ~1 GB Modelle. Ohne OpenRouter-Schlüssel funktionieren Aufnahme,
Transkription und Suche; Zusammenfassungen und Antworten brauchen einen Schlüssel oder ein lokales Modell.
Fragen, Live-Vorschläge und Zusammenfassung haben je ein eigenes Modell. Mit „Mit ChatGPT fortfahren“
in den Einstellungen läuft die Zusammenfassung über ein ChatGPT-Plus- oder Pro-Abo statt pro Token.

App-Bundle bauen: `scripts/bundle-macos.sh` → `dist/Live Notes.app`.

### Linux-Alpha

Unter Arch Linux/Omarchy das Paket aus dem Checkout bauen und installieren:

```bash
cd packaging/arch
makepkg -si
```

Den x86_64-Paketbuild gibt es zudem als `live-notes-arch-x86_64` in den CI-Artefakten.

Eine laufende PipeWire-/WirePlumber-Sitzung stellt Mikrofon und Systemton bereit. Für lokale
semantische Suche und lokale KI-Modelle muss `llama-server` aus llama.cpp im `PATH` liegen.
API-Schlüssel benötigen einen Secret-Service-Anbieter wie GNOME Keyring oder KWallet.
Für OCR lassen sich zusätzliche Tesseract-Sprachpakete wie `tesseract-data-deu` installieren.

Live Notes verwendet standardmäßig das Standardmikrofon und den Ton aller Ausgabegeräte, also
auch eines Headsets, das nicht die Standardausgabe ist. Später verbundene Ausgabegeräte werden
während der Aufnahme hinzugenommen. Mit
`wpctl status` und `wpctl inspect <ID>` kannst du den Knotennamen (`node.name`) oder die
Seriennummer (`object.serial`) für `LN_PIPEWIRE_MICROPHONE` oder `LN_PIPEWIRE_OUTPUT` ermitteln.
Für Ton aus einer einzelnen App kannst du `LN_PIPEWIRE_APPLICATION` auf den Knotennamen oder die
Seriennummer ihres aktiven Wiedergabestreams setzen. Dabei wird dieser Stream direkt abgegriffen.
Die Umgebungsvariablen werden beim Start einer Aufnahme gelesen. Ein Headset verhindert, dass die
Stimme der Gegenseite über Lautsprecher zusätzlich im Mikrofon landet.

Unter X11 startet und stoppt **Alt+Super+R** die Aufnahme. Unter Wayland meldet Live Notes
dieselbe Belegung als Vorschlag beim GlobalShortcuts-Portal an. Unter Hyprland lässt sich die
Portal-Aktion `works.karma.livenotes:toggle-recording` mit [einem globalen Tastenkürzel](https://wiki.hypr.land/configuring/core/binds/globals/#d-bus-global-shortcuts)
belegen. Das Tray-Symbol erscheint in einer Leiste mit StatusNotifier-Unterstützung. Im Fenster
funktioniert **Ctrl+Alt+R**.

Das Arch-Paket für x86_64 aktiviert Vulkan-Transkription. Whisper kann auf die CPU zurückfallen,
wenn keine verwendbare Vulkan-GPU verfügbar ist. Mit `LN_WHISPER_CPU=1` lässt sich die CPU
explizit wählen. Für den Build aus dem Checkout aktivieren Vulkan-Header und `glslc` zusammen mit
`cargo build --release --locked -p ln-app --features vulkan` die GPU-Unterstützung.

## Browser-Erweiterung und Bilder

Die Erweiterung für Zen und Firefox in [`extension/`](extension/README.md) speichert die aktuelle
Seite per Knopf, Kontextmenü oder Alt+Shift+L als Notiz: Webseiten mit Zusammenfassung und
Seitentext unter `Web/`, YouTube-Videos mit Zusammenfassung und Transkript (Zeitstempel springen
ins Video) unter `YouTube/`. Fehlt ein Transkript, holt die App Untertitel mit `yt-dlp` oder
transkribiert die Tonspur lokal mit Whisper. Die App nimmt dafür nur lokale Anfragen mit Token an
(Einstellungen → Browser-Erweiterung, Schnittstelle: [docs/clip-api.md](docs/clip-api.md)).

Bilder lassen sich per Drag & Drop oder Einfügen in eine Notiz übernehmen. Sie landen unter
`assets/` neben der Notiz und werden in der Leseansicht angezeigt.

## Aufbau

| Crate | Aufgabe |
|---|---|
| `ln-app` | Oberfläche (GPUI / gpui-kit), Menüleiste, Tastenkürzel |
| `ln-core` | Engine: Aufnahme-Sitzungen, Indexierung, Suche, Assistent, Live-Vorschläge |
| `ln-asr` | VAD, Spracherkennung, Whisper, OpenAI-Transkription, Nachbearbeitung |
| `ln-audio`, `ln-platform` | Aufnahme (Mikrofon mit Echo-Unterdrückung, Process Tap), OCR, PDF, Schlüsselbund |
| `ln-index`, `ln-ingest` | SQLite-Index (FTS5, sqlite-vec, RRF), Dateiformate, Chunking, Dateiüberwachung |
| `ln-ai`, `ln-models` | OpenAI-kompatibler Client, Prompts, Kosten; Modellkatalog, Downloads, llama-server |
| `ln-notes`, `ln-types` | Notizformat, Journal; gemeinsame Typen und Einstellungen |
| `ln-bench` | Messungen mit echten Modellen |

## Projektseite

`site/` ist die GitHub-Pages-Seite mit einer Live-Demo der App in HTML (Skill `.claude/skills/html-demo`),
veröffentlicht von `.github/workflows/pages.yml`.

## Dokumente

- [Spezifikation](docs/SPEC.md) · [Status](docs/STATUS.md)
- [Analyse von Google AI Edge Foresight](docs/foresight-analyse.md)
- [ADR-0001: UI-Framework](docs/adr/ADR-0001-ui-framework.md) · [ADR-0002: Modellanbindung](docs/adr/ADR-0002-modellanbindung.md)
