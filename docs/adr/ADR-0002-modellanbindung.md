# ADR-0002: Modellanbindung – austauschbar, online oder lokal

- Status: Vorschlag
- Datum: 2026-10-21

## Kontext

Live Notes muss nicht zwingend offline laufen. Ziel ist ein **sehr günstiges, multimodales Modell**,
das sich **leicht austauschen** lässt, plus eine **lokale Vektordatenbank**. Bevorzugt: OpenRouter,
ggf. LiteLLM. Die Gemma-Modelle sind wegen Größe, Preis und Multimodalität erste Wahl.

## Befund: Gemma auf OpenRouter (Abfrage der Modell-API am 2026-10-21)

| Modell | Eingabe | Kontext | Preis pro 1 Mio. Tokens (Ein/Aus) |
|---|---|---|---|
| `google/gemma-4-26b-a4b-it` | Text, Bild, Video | 262k | 0,09 $ / 0,30 $ |
| `google/gemma-4-26b-a4b-it:free` | Text, Bild, Video | 262k | 0 $ (Ratenlimit) |
| `google/gemma-4-31b-it` | Text, Bild, Video | 262k | 0,20 $ / 0,40 $ |
| `google/gemma-3-4b-it` | Text, Bild | 131k | 0,04 $ / 0,05 $ |

- **Gemma über OpenRouter: ja**, für Text, Bilder und Video.
- **Audio-Eingabe: nein.** Audio können nur die kleinen Varianten Gemma 4 E2B/E4B, und die bietet
  OpenRouter nicht an. Sie laufen nur lokal.
- **EmbeddingGemma gibt es auf OpenRouter nicht.** Mehrsprachige Alternativen dort:
  `qwen/qwen3-embedding-4b`/`-8b`, `baai/bge-m3`, `intfloat/multilingual-e5-large`,
  `google/gemini-embedding-001`.
- Günstige Modelle mit Audio-Eingabe auf OpenRouter: `google/gemini-2.5-flash-lite`,
  `mistralai/voxtral-small-24b-2507`, `qwen/qwen3.8-omni-flash`. Dazu der Endpunkt
  `/api/v1/audio/transcriptions` (Whisper).

Grobe Kosten mit `gemma-4-26b-a4b-it`: Zusammenfassung eines 1-Stunden-Meetings
(~15k Tokens rein, ~1k raus) ≈ **0,002 $**. Eine Frage mit 8 Chunks Kontext ≈ 0,0005 $.

## LiteLLM

LiteLLM ist ein Python-Proxy bzw. eine Python-Bibliothek. In einer nativen Rust-App müssten wir
eine Python-Laufzeit als Nebenprozess mitliefern (Hunderte MB, langsamer Start) – das widerspricht
dem Ziel „schnell und sparsam“. LiteLLM spricht aber selbst die OpenAI-API. Wer einen LiteLLM-Proxy
betreibt, kann ihn einfach als Endpunkt eintragen.

## Entscheidung (Vorschlag)

1. **Eine Schnittstelle: OpenAI-kompatible API** (`/chat/completions`, `/embeddings`,
   `/audio/transcriptions`). Ein Rust-Client mit einstellbarer Basis-URL, API-Schlüssel und Modell-ID.
   Damit funktionieren ohne Sonderlogik: OpenRouter, LiteLLM-Proxy, OpenAI, Ollama, LM Studio,
   llama.cpp-Server, vLLM.
2. **Modell pro Aufgabe („Rolle“)**, jeweils frei wählbar:

| Rolle | Standard | Alternativen |
|---|---|---|
| Transkription (live) | **lokal**: Whisper large-v3-turbo (whisper.cpp) | Cloud: OpenAI `gpt-live-transcribe` (Realtime-API) |
| Transkription (Datei/Nachbearbeitung) | **lokal**: Whisper | Cloud: OpenAI `gpt-transcribe`, OpenRouter `/audio/transcriptions`, Gemini Flash-Lite (Audio) |
| Assistent (Zusammenfassung, Fragen, Notiz-Anreicherung) | **OpenRouter**: `google/gemma-4-26b-a4b-it` | `gemma-4-31b-it`, jedes andere OpenRouter-Modell; lokal Gemma 4 E2B/E4B |
| Bildverständnis (Bilder/Screenshots beim Indexieren) | **lokal**: Gemma 4 E2B/E4B (llama-server) | Cloud: `google/gemma-4-26b-a4b-it` oder jedes Modell mit Bildeingabe |
| Embeddings | **lokal**: EmbeddingGemma 300M | Cloud: `qwen/qwen3-embedding-4b`, `baai/bge-m3` |

3. **Lokale Modelle laufen über denselben Client.** Die App startet bei Bedarf einen mitgelieferten
   `llama-server` (llama.cpp) auf `127.0.0.1` und spricht ihn wie OpenRouter an. Vorteil: ein Codepfad,
   und Beenden des Prozesses gibt den Speicher sofort frei. Ausnahme ist Whisper: läuft im Prozess
   (`whisper-rs`), weil die Live-Transkription Zugriff auf Spracherkennung, Zeitstempel und
   Streaming braucht.
4. **Embeddings bleiben standardmäßig lokal.** Gründe: Der Index ist an ein Embedding-Modell gebunden
   (Wechsel = Neuindexierung), soll offline durchsuchbar sein, beim Indexieren großer Ordner fallen viele
   Anfragen an, und Dateiinhalte verlassen so nicht den Rechner. EmbeddingGemma ist mit 300M Parametern
   auch auf einem M1 mit 8 GB schnell genug.
5. **Transkription bleibt standardmäßig lokal**, ist aber per Einstellung auf die Cloud umschaltbar.
   Gründe für lokal als Standard: Kosten im Dauerbetrieb, Datenschutz und volle Kontrolle über die
   Spracherkennung pro Abschnitt. Für die OpenAI-Realtime-API (`gpt-live-transcribe`) gibt es einen
   zweiten Adapter `OpenAiRealtime` (WebSocket), weil sie nicht über die normale OpenAI-kompatible
   REST-Schnittstelle läuft. `gpt-transcribe` nutzt den REST-Endpunkt `/v1/audio/transcriptions`.
   Aktiviert wird OpenAI über einen **eigenen OpenAI-API-Schlüssel** (getrennt vom OpenRouter-
   Schlüssel). Kosten pro Gesprächsstunde: lokal ~0,03 $, `gpt-transcribe` ~0,26 $,
   `gpt-live-transcribe` ~0,88 $ (Details SPEC §9.5).
6. **Bildverständnis standardmäßig lokal** (Gemma 4 E2B/E4B), weil Bilder oft private Inhalte
   (Screenshots, Dokumente) zeigen. Läuft gesammelt im Leerlauf, nie parallel zu einer Aufnahme auf
   Geräten mit wenig RAM.

## Konsequenzen

- Auf dem 8-GB-Referenz-Mac läuft der Assistent in der Cloud; das lokale Gemma 4 E2B wird nur für
  Bildverständnis gebraucht und nachgeladen, sobald eine Quelle Bilder enthält.
- Der Assistent braucht Internet und einen OpenRouter-Schlüssel (oder einen lokalen Endpunkt).
  Suche und Transkription funktionieren weiterhin offline.
- Transkripte und Notizen gehen beim Fragen/Zusammenfassen an den Anbieter. Das muss in der UI klar
  sichtbar und pro Rolle abschaltbar sein (siehe SPEC §11).
- Modellwechsel ist eine Einstellung, kein Code. Für Embeddings mit Hinweis auf Neuindexierung.
