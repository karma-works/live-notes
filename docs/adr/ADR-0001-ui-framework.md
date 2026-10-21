# ADR-0001: UI-Framework

- Status: Vorschlag
- Datum: 2026-10-21

## Kontext

Anforderungen an die UI:

1. Rein nativ (kein WebView, kein Electron/Tauri-Webfrontend), Rust.
2. Sehr schnell, niedriger Speicherverbrauch, ~0 % CPU im Leerlauf.
3. Schön anzusehen (moderne Typografie, Animationen, Dark Mode).
4. Textlastig: lange Transkripte, Markdown-Notizen, Chat, Suchergebnisse.
5. **Mehrsprachiger Text inkl. Chinesisch**: korrekte Schriftglättung, Font-Fallback, Text-Shaping
   und **IME-Eingabe** (Pinyin-Eingabe in Suchfeld und Notizen).
6. macOS-Integration: Menüleisten-Icon, Dateidialoge, Berechtigungen, globale Tastenkürzel.

Zielplattform ist vorerst nur macOS (Process Tap für System-Audio ist macOS-spezifisch).

## Untersuchte Optionen

### Raylib (raylib 6.0, Crate `raylib` 6.0.0)

Raylib ist eine C-Bibliothek für Spiele und Grafik-Lernprojekte, 6.0 erschien im April 2026
(neu: Software-Renderer, neue Text- und Dateisystem-API).

Stärken:
- Sehr klein, einfach, kaum Abhängigkeiten, gute Rust-Bindings.
- Geringer Speicherbedarf, schnelle Startzeit.
- Gut für eigene Visualisierungen (Wellenform, Pegel, Animationen).

Schwächen für unser Projekt:
- **Kein Text-Shaping und kein echter Font-Fallback.** Glyphen werden in einen Atlas vorgerendert;
  für Chinesisch müssten Tausende Hanzi vorab oder dynamisch selbst verwaltet werden.
  Komplexe Schriften (Arabisch, Thai, Hindi) werden falsch dargestellt.
- **Keine IME-Unterstützung** über GLFW hinaus: Pinyin-Eingabe mit Kandidatenfenster funktioniert
  nicht ordentlich. Für ein Notiz-Tool mit Chinesisch-Fokus ein K.-o.-Kriterium.
- Kein Text-Editing-Widget mit Auswahl, Undo, Cursor-Navigation; `raygui` ist sehr rudimentär.
- Rendering über OpenGL 3.3, das auf macOS veraltet ist (läuft über Apples GL-auf-Metal-Schicht).
- Spieleschleife: zeichnet standardmäßig mit 60 FPS; `EnableEventWaiting` hilft, ist aber nicht das
  Grundmodell.
- Keine Barrierefreiheit (VoiceOver), keine nativen Menüs, Dialoge, Tray-Icons.

Bewertung: Für eine textlastige Produktivitäts-App ungeeignet. Wir müssten Textsystem, Editor,
Layout und Widgets selbst bauen. Raylib lohnt sich höchstens für eine einzelne Visualisierung –
das kann aber auch das gewählte Framework.

### GPUI (Zed) + gpui-component

- Rust, rendert direkt über **Metal**, entwickelt für den Zed-Editor (hohe Last mit großen Texten).
- Textsystem über CoreText: Shaping, Font-Fallback, CJK, **IME** (Zed unterstützt Pinyin-Eingabe).
- Zeichnet nur bei Änderungen, Leerlauf nahe 0 % CPU.
- Tailwind-ähnliches Styling, flüssige Animationen → leicht schön zu gestalten.
- `gpui-component` (Longbridge) liefert 60+ Widgets: Eingabefeld, virtuelle Listen, Tabellen,
  Markdown-Ansicht, Dock-Layout, Themes.
- Schwächen: pre-1.0, Breaking Changes, dünne Doku. Status-Item (Menüleiste) ggf. direkt über
  `objc2-app-kit`.

### Iced

- Elm-Architektur, `wgpu`-Renderer, Text über `cosmic-text` (Shaping, Fallback).
- Stabiler und besser dokumentiert als GPUI, aber IME und Text-Editing sind weniger ausgereift,
  und die Optik wirkt ohne viel Eigenarbeit schlichter.

### egui

- Immediate-Mode, sehr produktiv für Tools und Debug-Oberflächen.
- Optik eher „Entwickler-Tool“; Text-Layout und IME eingeschränkt; zeichnet bei Interaktion jedes Frame neu.

### Slint

- Deklarative DSL, sehr sparsam (auch Embedded), gute Tools.
- Lizenz (GPL/Royalty-free/kommerziell) prüfen; komplexe Textansichten (lange Transkripte mit Auswahl)
  weniger flexibel.

### AppKit direkt über `objc2`

- Maximal nativ, beste Systemintegration, kleinster Speicher.
- Sehr viel Boilerplate, unsafe-lastig, langsame UI-Entwicklung in Rust.

## Vergleich

| Kriterium | GPUI | Iced | egui | Slint | AppKit/objc2 | Raylib |
|---|---|---|---|---|---|---|
| Rendering | Metal | wgpu/Metal | wgpu/GL | Skia/femtovg | AppKit | OpenGL 3.3 |
| CJK + Shaping | ✅ CoreText | ✅ cosmic-text | ⚠️ | ✅ | ✅ | ❌ |
| IME | ✅ | ⚠️ | ⚠️ | ✅ | ✅ | ❌ |
| Leerlauf-CPU | ✅ | ✅ | ⚠️ | ✅ | ✅ | ⚠️ |
| Optik ohne Mehraufwand | ✅ | ⚠️ | ⚠️ | ✅ | ✅ (System-Look) | ❌ |
| Widget-Vorrat | ✅ (gpui-component) | ✅ | ✅ | ✅ | ✅ | ❌ |
| API-Stabilität | ⚠️ pre-1.0 | ⚠️ pre-1.0 | ✅ | ✅ | ✅ | ✅ |
| Entwicklungstempo | ✅ | ✅ | ✅✅ | ✅ | ❌ | ❌ |

## Entscheidung (Vorschlag)

**GPUI + gpui-component**, ergänzt um `objc2-app-kit` für Menüleisten-Icon, Dateidialoge und
Berechtigungsabfragen.

Absicherung gegen das pre-1.0-Risiko:
- GPUI auf einen festen Git-Commit bzw. eine feste Crate-Version pinnen.
- Die UI ist eine dünne Schicht (`ln-app`). Die gesamte Logik liegt in UI-freien Crates mit eigener
  Fassade (`ln-core`). Ein Wechsel auf Iced bliebe so auf die UI-Crate begrenzt.
- In Meilenstein 0 ein Spike: Transkript-Liste mit 10.000 Zeilen, chinesische Pinyin-Eingabe,
  Markdown-Leseansicht (GFM-Tabellen, Aufgabenlisten, Code mit Hervorhebung) und Markdown-Editor
  (10.000 Zeilen, IME, Undo), Speicher/CPU messen. Fällt der Spike durch → Iced.

## Konsequenzen

- Sehr gute Performance und Typografie, insbesondere für Chinesisch.
- Wir akzeptieren Upgrade-Aufwand bei GPUI-Versionssprüngen.
- Plattform zunächst nur macOS; GPUI unterstützt inzwischen auch Linux/Windows, das bleibt eine Option.
