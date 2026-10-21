# Projektseite (GitHub Pages)

Statische Seite ohne Build-Schritt. `.github/workflows/pages.yml` veröffentlicht diesen Ordner bei jedem Push
auf `main` (einmalig: Repository → Settings → Pages → Source „GitHub Actions“).

- `index.html`: die Seite.
- `demo.js`, `demo.css`: die App als Live-HTML-Demo (Skill `html-demo`, Präfix `lnx-`), vier Schritte von der
  Aufnahme bis zur Zusammenfassung. Bezeichnungen aus `crates/ln-app/locales/de.ftl`, Layout nach
  `docs/screenshots/`, Transkript aus einem echten Prüflauf (`scripts/verify.sh tour`).
- `step-window.js`, `step-window.css`: Kopie der Engine aus `.claude/skills/html-demo/scripts/`, ergänzt um
  `title` für ein natives App-Fenster.

Lokal ansehen: `python3 -m http.server 8765 --directory site`, dann im Browser `window.demo.seek(12)` zum Springen.

**Offen:** Die Zusammenfassung im vierten Schritt (`SUMMARY` in `demo.js`) ist von Hand aus dem Transkript
geschrieben. Sie soll durch die echte Ausgabe der App ersetzt werden (Zusammenfassung des Prüflauf-Transkripts mit
Gemma 4 oder dem ChatGPT-Abo).
