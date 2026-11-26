# Live Notes – Browser-Extension (Zen / Firefox)

Speichert den aktuellen Tab als Notiz in Live Notes:

- **Werkzeugleisten-Knopf**, **Kontextmenü „In Live Notes speichern“** oder **Alt+Umschalt+L**.
- Normale Seiten: Artikeltext über Mozillas Readability (Titel, Autor, Sprache, Text, HTML).
  Ist Text markiert, wird nur die Markierung zusammengefasst.
- YouTube (`/watch`, `/shorts/`, `youtu.be`): Metadaten und Transkript direkt aus der Seite mit
  der eigenen YouTube-Sitzung. Gibt es kein Transkript, holt die App es selbst (yt-dlp/Whisper).
- Das Symbol zeigt den Zustand: `…` sendet/arbeitet, `✓` gespeichert, `!` Fehler. Am Ende
  kommt eine Benachrichtigung mit dem Titel der Notiz.

Die Schnittstelle zur App ist in [`../docs/clip-api.md`](../docs/clip-api.md) beschrieben
(nur `127.0.0.1`, Token, Standardport 47823).

## Einrichten

1. Live Notes starten, unter *Einstellungen → Browser-Extension* den Token kopieren.
2. Extension installieren (siehe unten). Die Einstellungen öffnen sich beim ersten Mal von
   selbst, sonst `about:addons` → Live Notes → *Einstellungen*.
3. Token einfügen, Port nur ändern, wenn er in der App geändert wurde, **Verbindung testen**.
4. Falls Zen nach dem Zugriff auf `127.0.0.1` / `www.youtube.com` fragt: erlauben. Ohne diese
   Berechtigung erreicht die Extension die App nicht.

Das Tastenkürzel lässt sich unter `about:addons` → Zahnrad → *Tastenkürzel verwalten* ändern.

## Signieren und dauerhaft installieren

Zen installiert dauerhaft nur signierte Extensions. „Unlisted“ heißt: Mozilla signiert sie,
sie erscheint aber nicht öffentlich auf addons.mozilla.org (AMO).

1. **AMO-API-Schlüssel holen:** mit dem Firefox-Konto bei
   <https://addons.mozilla.org/developers/addon/api/key/> anmelden und „Neue Zugangsdaten
   erstellen“. Man bekommt einen *JWT-Aussteller* (= API-Key, `user:…`) und ein *JWT-Geheimnis*
   (= API-Secret). Das Geheimnis gehört nicht ins Repository und nicht in Dateien im Projekt.
2. **Signieren** (im Ordner `extension/`):

   ```sh
   npm ci
   export WEB_EXT_API_KEY='user:12345678:123'
   read -rs WEB_EXT_API_SECRET && export WEB_EXT_API_SECRET   # Geheimnis eintippen
   ./scripts/sign.sh
   ```

   Das Skript baut `dist/`, prüft es mit `web-ext lint` und lädt es zur Signierung hoch. Die
   signierte Datei landet in `extension/web-ext-artifacts/live_notes-<version>.xpi`. Die erste
   Signierung einer neuen ID kann einige Minuten dauern. Für jede neue Signierung muss
   `version` in `src/manifest.json` erhöht werden.
3. **In Zen installieren:** `about:addons` öffnen → Zahnrad → *Add-on aus Datei installieren…*
   → die `.xpi` wählen (oder die Datei ins Zen-Fenster ziehen) → *Hinzufügen*.

## Entwicklung

```sh
npm ci
npm test            # Unit-Tests (node:test): Transkript-Parser, Payloads, API-Client
npm run lint        # baut dist/ und prüft mit web-ext lint
npm run start       # Zen mit frischem Wegwerf-Profil und geladener Extension
npm run mock        # Mock der Live-Notes-API auf :47899, Token "test-token"
npm run test:live   # prüft die YouTube-Strategien live gegen youtube.com (Netzwerk!)
npm run test:e2e    # Ende-zu-Ende: Mock + Testbuild in Zen (headless, Wegwerf-Profil)
```

**Temporär laden ohne Signatur:** `npm run build`, dann in Zen `about:debugging#/runtime/this-firefox`
→ *Temporäres Add-on laden…* → `extension/dist/manifest.json` wählen. Die Extension bleibt bis
zum Neustart von Zen geladen; nach Änderungen dort auf *Neu laden* klicken. Hintergrund-Logs
(z. B. welche Transkript-Strategie gegriffen hat) zeigt dort *Untersuchen*.

Für die lokale Entwicklung ohne laufende App: `npm run mock` starten und in den Extension-
Einstellungen Port `47899` und Token `test-token` eintragen.

### Aufbau

```
src/manifest.json          MV3, gecko.id live-notes@karma.works, Firefox ≥ 140
src/background.js          Event-Page: Knopf, Menü, Tastenkürzel → saveTab(), Polling, Badge
src/lib/api.js             Client für /v1/ping, /v1/clip, /v1/clip/{id} inkl. Fehlercodes
src/lib/payload.js         baut die POST-/v1/clip-Bodies, hält sie unter 8 MB
src/lib/youtube.js         YouTube-Logik (läuft in der Seite und unter Node für Tests)
src/content/page-extract.js   Readability-Auswertung in der Seite
src/content/youtube-main.js   läuft in der MAIN-World von youtube.com (Cookies der Sitzung)
src/options/               Einstellungen (Port, Token, Verbindung testen)
src/_locales/{de,en}/      Deutsch, Englisch als Rückfall
src/icons/                 icon.svg + PNGs (npm run icons erzeugt die PNGs neu)
scripts/build.mjs          kopiert src/ → dist/ und vendort Readability.js (Apache-2.0)
scripts/sign.sh            Build + web-ext sign --channel unlisted
test/                      Unit-Tests, Fixtures, Mock-Server, Live- und E2E-Test
test/test-hook.js          NUR im Testbuild (dist-test/), nie in dist/
```

### YouTube-Transkript: Reihenfolge der Strategien

1. **innertube** – `youtubei/v1/get_panel` mit `panelId: "PAmodern_transcript_view"`, so wie
   youtube.com selbst seit dem neuen Transkript-Panel. Die `params` stammen aus `ytInitialData`
   (`showEngagementPanelEndpoint`) oder werden aus der Video-ID gebaut, wenn `ytInitialData`
   nach einer In-App-Navigation veraltet ist. `context.client.hl` wird auf die Sprache der
   Originalspur gesetzt, sonst liefert YouTube die Spur in der Oberflächensprache.
   Danach das ältere `get_transcript` (liefert seit dem Umbau meist `FAILED_PRECONDITION`).
2. **panel** – „Transkript anzeigen“ in der Seite öffnen und die Segmente aus dem DOM lesen
   (zeigt die Spur in der Oberflächensprache).
3. **timedtext** – `captionTracks[].baseUrl` + `&fmt=json3`; ist ohne PO-Token leer, das ist
   erwartet.

Scheitern alle, geht `transcript: null` an die App.

### Lizenz von Readability

`vendor/Readability.js` im Build ist eine unveränderte Kopie von `@mozilla/readability`
(Apache-2.0); die Lizenz liegt daneben als `vendor/Readability.LICENSE.md`.
