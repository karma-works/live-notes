# Clip API (Browser-Extension → Live Notes)

Lokaler HTTP-Endpunkt, über den die Zen/Firefox-Extension Webseiten und YouTube-Videos als
Notiz ablegt. Bewusst schlichtes HTTP+JSON statt MCP: die Extension ist kein LLM-Client. Die
eigentliche Logik liegt in `ln-core` (`Core::clip`), damit auch der MCP-Server unter `/mcp` desselben Endpunkts sie als Werkzeug
`clip_url` nutzt ([docs/mcp.md](mcp.md)).

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

- Ablage wie Aufnahmen: `<Notizordner>/YYYY/MM/YYYY-MM-DD HHMM <Titel>.md` (`unique_path` bei
  Kollision), damit die Notizliste sie unter „Heute“ usw. zeigt.
- Front Matter mit `created`, `source` (URL), `source_kind` (`page`/`youtube`), `labels`
  (`[Web, <Host>]` bzw. `[YouTube]`, in der Leseansicht als `#Web #example.com`), bei YouTube
  `channel` und `duration`. Notizen mit `source_kind` gelten in der Liste nicht als Aufnahme.
- Inhalt: `# Titel`, Quelllink, `## Zusammenfassung` (KI, Sprache = Inhaltssprache bzw.
  `summary_language`), Kernpunkte. Bei YouTube darunter `## Transkript` im bestehenden
  Transkriptformat von `ln-notes`, jeder Zeitstempel verlinkt auf `…&t=<s>s`.
- Ohne verfügbares Zusammenfassungsmodell wird die Notiz trotzdem geschrieben (Seitentext bzw.
  Transkript), mit Hinweis statt Zusammenfassung.

## Umsetzung (App-Seite)

Ergänzungen und Präzisierungen gegenüber dem Entwurf oben, so wie `ln-core` (`crates/ln-core/src/clip/`) sie umsetzt:

- **Fehlerantworten** haben den Body `{"error":"…"}`: `400` (ungültiges JSON oder ungültige Anfrage,
  z. B. keine http(s)-URL, Seite ohne `text`/`html`/`selection`, YouTube ohne gültige `video_id`),
  `401` (Token fehlt/falsch), `403` (fremder `Origin` oder ein `Host`, der nicht `127.0.0.1`,
  `localhost` oder `[::1]` ist – Schutz gegen DNS-Rebinding), `404` (unbekannter Pfad oder Job),
  `405`, `413` (Body > 8 MB).
- `OPTIONS` liefert `204` ohne Token (Origin-Prüfung gilt trotzdem). Antworten an Extension-Origins
  tragen `Access-Control-Allow-Origin` mit genau diesem Origin.
- `youtube.video_id` darf fehlen, wenn sie aus der URL hervorgeht (`watch?v=`, `youtu.be/`,
  `shorts/`, `embed/`, `live/`). Leere Transkripteinträge werden verworfen, der Rest nach `start_ms`
  sortiert.
- **Jobstatus**: Felder ohne Wert fehlen im JSON. `stage` ist eines von `queued`,
  `fetching_transcript`, `downloading_audio`, `waiting_for_recording` (lokale Transkription wartet,
  bis eine laufende Aufnahme endet), `transcribing`, `summarizing`, `writing`. Beim Transkribieren
  gibt es zusätzlich `progress` (0…1). Jobs liegen nur im Speicher (die letzten 200 abgeschlossenen)
  und sind nach einem Neustart der App unbekannt (`404`).
- **Token**: Secret-Store-Eintrag `clip-token` (Dienst `works.karma.livenotes`). Ohne Secret Store
  gilt ein zufälliger Token nur bis zum Beenden; die Einstellungen weisen darauf hin.
- **Untertitel** ohne Transkript der Extension: `yt-dlp -J` liefert Titel, Kanal, Dauer,
  Beschreibung und die Spuren. Gewählt wird: Autoren-Untertitel in der Videosprache bzw.
  `transcript_lang`/`lang`/`en`/`de` → YouTubes Spracherkennung in Originalsprache (`*-orig`) →
  automatische Untertitel in diesen Sprachen → beliebige Autoren-Untertitel. Format json3, sonst VTT
  (rollende Auto-Untertitel werden entdoppelt). Ohne Untertitel: `yt-dlp -x` → 16 kHz Mono-WAV
  (ffmpeg) → Whisper über den gemeinsamen Transkriptions-Worker der App. Fehlen `yt-dlp` oder
  `ffmpeg`, endet der Job mit `error` und einer Installationsanleitung. `LN_YTDLP`/`LN_FFMPEG`
  überschreiben die Programmpfade.
- **Zusammenfassung**: eigenes Prompt (`ln_ai::prompts::web_summary`): Titel, 3–6 Sätze,
  Kernpunkte, Sprache = `summary_language`, sonst `lang`/Videosprache, sonst Sprache des Inhalts.
  Inhalte über 48 000 Zeichen werden in Teilen zu je 24 000 Zeichen vorgelesen (höchstens 8 Teile),
  danach zusammengefasst. Für Seiten nur `selection`, wenn gesetzt; `html` wird nur ohne `text`
  verwendet (als reiner Text).
- **Notiz**: Titel = Seiten-/Videotitel, nur ohne ihn der Titel des Modells. Front Matter zusätzlich
  `id`, `title`, `languages`, bei YouTube `transcription` (`youtube/captions`,
  `youtube/auto-captions` oder das Whisper-Modell), `app`. Unter `# Titel` eine Quellzeile
  (`[example.com](url) · Website · Autor` bzw. `[YouTube](url) · Kanal · 3:33`). `## Zusammenfassung`
  enthält die Sätze und `### Kernpunkte`. Seiten bekommen danach `## Seitentext` bzw.
  `## Markierter Text` (Marker `<!-- ln:source -->`), damit Suche und Fragen den Inhalt finden.
  YouTube-Transkriptzeilen: `[HH:MM:SS](https://www.youtube.com/watch?v=<id>&t=<s>s) <Kanal> (<lang>): <Text>`;
  kurze Untertitel-Cues werden zu Zeilen von etwa 15–40 s zusammengefasst. Die Leseansicht macht den
  Zeitstempel klickbar, in anderen Markdown-Programmen ist er ein normaler Link.
- Ohne Zusammenfassungsmodell steht unter `## Zusammenfassung` ein Hinweis
  (`> **Keine Zusammenfassung:** <Grund>`).
- Headless zum Testen: `LN_SECRETS=memory LN_DATA_DIR=… LN_NOTES_DIR=… live-notes --serve-clip`
  startet nur den Endpunkt und gibt Status und Token als JSON aus. `LN_CLIP_SKIP_SUBS=1` überspringt
  Untertitel, um den Weg über Tonspur und Whisper zu prüfen.
