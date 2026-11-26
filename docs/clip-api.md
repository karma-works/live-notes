# Clip API (Browser-Extension → Live Notes)

Lokaler HTTP-Endpunkt, über den die Zen/Firefox-Extension Webseiten und YouTube-Videos als
Notiz ablegt. Bewusst schlichtes HTTP+JSON statt MCP: die Extension ist kein LLM-Client. Die
eigentliche Logik liegt in `ln-core` (`Core::clip`), damit ein späterer MCP-Server sie als Tool
wiederverwenden kann.

## Transport und Sicherheit

- Live Notes lauscht nur auf `127.0.0.1`, Standardport **47823** (Einstellung `clip.port`).
- Jede Anfrage braucht `Authorization: Bearer <token>`. Den Token erzeugt die App beim ersten
  Start (32 Byte Zufall, hex), speichert ihn im Secret Store und zeigt ihn in den Einstellungen
  („Browser-Extension“) mit Kopieren- und Neu-erzeugen-Knopf.
- Anfragen mit einem `Origin`, der nicht mit `moz-extension://` oder `chrome-extension://`
  beginnt, werden mit 403 abgewiesen (Schutz vor Webseiten, die localhost ansprechen).
- Body-Limit 8 MB. Die Extension sendet nur über `host_permissions` für `http://127.0.0.1/*`,
  daher ist kein CORS nötig; `OPTIONS` beantwortet der Server trotzdem mit 204.
- Abschaltbar in den Einstellungen (`clip.enabled`, Standard an).

## Endpunkte

### `GET /v1/ping`
`200 {"app":"live-notes","version":"1.x.y","api":1}` bei gültigem Token, sonst `401`.

### `POST /v1/clip`
```json
{
  "kind": "page" | "youtube",
  "url": "https://…",
  "title": "Seitentitel",
  "site_name": "optional",
  "byline": "optional, Autor",
  "lang": "optional, BCP-47 der Seite, z. B. de",
  "text": "page: Artikeltext aus Readability (textContent), Absätze durch \n\n getrennt",
  "html": "optional, page: Readability content (HTML) – die App darf daraus Markdown machen",
  "selection": "optional: markierter Text; wenn gesetzt, wird nur dieser zusammengefasst",
  "youtube": {
    "video_id": "dQw4w9WgXcQ",
    "channel": "optional",
    "duration_s": 213,
    "description": "optional",
    "transcript_lang": "optional, z. B. de",
    "transcript_source": "innertube" | "panel" | "timedtext",
    "transcript": [ { "start_ms": 0, "dur_ms": 4200, "text": "…" } ]
  }
}
```
`youtube.transcript` darf `null`/fehlen: dann holt die App selbst Untertitel über
`yt-dlp --write-subs --write-auto-subs --skip-download`, und wenn es keine gibt, lädt sie die
Tonspur mit `yt-dlp -x` und transkribiert sie lokal mit Whisper (`ln-asr`).

Antwort `202 {"job_id":"…"}`. Die App arbeitet im Hintergrund weiter.

### `GET /v1/clip/{job_id}`
`200 {"status":"pending"|"done"|"error","stage":"summarizing|transcribing|…","path":"/abs/pfad.md","title":"…","error":"…"}`

## Ergebnis-Notiz

- Ablage: `<Notizordner>/Web/YYYY-MM-DD <Titel>.md` bzw. `<Notizordner>/YouTube/YYYY-MM-DD <Titel>.md`
  (`unique_path` bei Kollision).
- Front Matter mit `created`, `source` (URL), `source_kind` (`page`/`youtube`), bei YouTube
  `channel` und `duration`.
- Inhalt: `# Titel`, Quelllink, `## Zusammenfassung` (KI, Sprache = Inhaltssprache bzw.
  `summary_language`), Kernpunkte. Bei YouTube darunter `## Transkript` im bestehenden
  Transkriptformat von `ln-notes`, jeder Zeitstempel verlinkt auf `…&t=<s>s`.
- Ohne verfügbares Zusammenfassungsmodell wird die Notiz trotzdem geschrieben (Seitentext bzw.
  Transkript), mit Hinweis statt Zusammenfassung.
