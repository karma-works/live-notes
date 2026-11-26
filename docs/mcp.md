# MCP-Server (Claude Code, Claude Desktop → Live Notes)

Live Notes stellt seine Notizen MCP-Clients als Werkzeuge bereit. Der Server läuft im
lokalen Clip-Endpunkt ([docs/clip-api.md](clip-api.md)) unter dem Pfad **`/mcp`**.

## Warum Streamable HTTP im Clip-Endpunkt und kein `live-notes --mcp` (stdio)

- **Ein Prozess, ein Index.** Die Werkzeuge rufen dieselbe `Core`-Instanz auf wie die offene App:
  derselbe SQLite-Index, dieselbe Dateiüberwachung, dasselbe Zusammenfassungs- und
  Einbettungsmodell (ggf. ein laufender `llama-server`), dieselben Clip-Jobs. Ein stdio-Server
  wäre ein zweiter Prozess, der den Index parallel öffnet, eigene Indexer und Modelle startet
  und von dem die App erst über Dateiereignisse erfährt.
- **Kein neuer Angriffsweg.** Bindung an `127.0.0.1`, Bearer-Token, Prüfung von `Origin` und
  `Host` (DNS-Rebinding) und das 8-MB-Limit gelten unverändert, wie es die MCP-Spezifikation für
  Streamable HTTP verlangt.
- **Weniger Code.** Kein zweiter Startpfad ohne Fenster; `clip_url` läuft über `Core::clip`.

Nachteil: Die App (oder `live-notes --serve-clip`) muss laufen, und der Client braucht den Token.

## Einrichten in Claude Code

1. Live Notes starten. Unter **Einstellungen → Browser-Erweiterung** den Token kopieren (es ist
   derselbe wie für die Extension) und prüfen, dass der Endpunkt läuft (Standardport 47823).
2. Server eintragen (`--scope user` für alle Projekte, sonst nur das aktuelle):

```bash
claude mcp add --transport http --scope user live-notes http://127.0.0.1:47823/mcp --header "Authorization: Bearer <token>"
```

3. `claude mcp list` zeigt `live-notes: … ✓ Connected`. In einer Sitzung zeigt `/mcp` die
   Werkzeuge.

Wird der Token in den Einstellungen neu erzeugt, den Eintrag mit `claude mcp remove live-notes`
entfernen und neu anlegen. Ohne Secret Store ändert sich der Token bei jedem Start (die
Einstellungen weisen darauf hin).

**Claude Desktop** bindet lokale Server über stdio ein; dafür vermittelt `mcp-remote`
(`claude_desktop_config.json`):

```json
{ "mcpServers": { "live-notes": { "command": "npx", "args": ["-y", "mcp-remote",
  "http://127.0.0.1:47823/mcp", "--header", "Authorization: Bearer <token>"] } } }
```

## Werkzeuge

Pfade sind relativ zum Notizordner und mit `/` getrennt, so wie `search_notes` und
`create_note` sie liefern. Fehler kommen als Werkzeugergebnis mit `isError: true` und einem
lesbaren Grund zurück; jedes Ergebnis steht als JSON im Text und in `structuredContent`.

| Werkzeug | Argumente | Ergebnis |
|---|---|---|
| `search_notes` | `query`, `limit` (1–50, Standard 10), `folder` | `results: [{path, title, snippet, in_notes_folder}]` – hybride Suche (FTS5 + Einbettungen) über `ln-index`, gruppiert nach Datei, wie die Suchleiste der App. Auch Dateien aus weiteren indexierten Ordnern; die haben `in_notes_folder: false` und absolute Pfade. |
| `read_note` | `path` | `{path, title, content}` – die ganze Markdown-Datei samt Front Matter, höchstens 200 000 Zeichen (dann `truncated`). Nur `.md`-Dateien im Notizordner; `..` und Symlinks nach außen werden abgewiesen. |
| `create_note` | `title`, `content`, `folder` | `{path, title}` – neue Datei `<folder>/<Titel>.md` mit Front Matter (`id`, `title`, `created`, `app`), `# Titel` und dem Inhalt. Überschreibt nie (`unique_path`), legt den Ordner an, `folder` darf weder absolut sein noch `..` oder versteckte Teile enthalten. |
| `clip_url` | `url`, `title`, `text`, `lang`, `wait_seconds` (0–600, Standard 90) | `{job_id, status, stage?, path?, title?}` – wie die Extension (`Core::clip`): YouTube-URLs werden mit Untertiteln bzw. lokaler Transkription gespeichert, sonst eine Webseite. Ohne `text` lädt die App die Seite selbst (30 s, 8 MB, nur HTML/Text) und nimmt `<article>`, sonst `<main>`, sonst `<body>` ohne Navigation, Kopf- und Fußzeilen. Ist die Notiz nach `wait_seconds` nicht fertig, kommt `status: pending` mit `job_id`. Ein fehlgeschlagener Job ist ein Werkzeugfehler. |
| `clip_status` | `job_id` | wie `clip_url`; Jobs liegen nur im Speicher und sind nach einem Neustart unbekannt. |

## Protokoll

- `POST /mcp` mit genau einer JSON-RPC-Nachricht. Anfragen bekommen `200` mit
  `application/json`; Benachrichtigungen und Antworten `202` ohne Inhalt. Kein SSE-Stream, keine
  `Mcp-Session-Id`: `GET` und `DELETE` liefern `405`. Batches (Arrays) → JSON-RPC `-32600`.
- Revisionen `2025-11-25`, `2025-06-18`, `2025-03-26`, `2024-11-05`. `initialize` antwortet mit der
  angefragten, wenn bekannt, sonst mit der neuesten. Ein unbekannter `MCP-Protocol-Version`-Header
  → `400`. Unterstützt: `initialize`, `ping`, `tools/list`, `tools/call`.
- Ungültiges JSON → `400` mit JSON-RPC `-32700`; Token/Origin/Host wie beim Clip-API (`401`/`403`).

## Ausprobieren ohne Fenster

```bash
LN_SECRETS=memory LN_DATA_DIR=/tmp/ln-mcp/data LN_NOTES_DIR=/tmp/ln-mcp/notes live-notes --serve-clip
```

gibt `{"status":"Running { port: 47823 }","token":"…"}` aus; danach wie oben `claude mcp add`.
