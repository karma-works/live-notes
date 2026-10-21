# Live Notes

Native macOS-App in Rust: Gespräche mitschneiden und **in der gesprochenen Sprache** transkribieren,
lokale Ordner indexieren und durchsuchen, Fragen an das eigene Wissen stellen.

Transkription und Vektorsuche laufen lokal. Der Assistent nutzt ein günstiges, multimodales Modell
(Standard: Gemma 4 über OpenRouter), austauschbar über jede OpenAI-kompatible API (OpenRouter,
LiteLLM-Proxy, Ollama, lokales Gemma). Keine Google-Drive- oder sonstige Cloud-Datenquellen.

Status: Spezifikationsphase. Lizenz: [Apache-2.0](LICENSE). Plattform: macOS (Apple Silicon), Linux geplant.

- [Spezifikation](docs/SPEC.md)
- [Analyse von Google AI Edge Foresight](docs/foresight-analyse.md)
- [ADR-0001: UI-Framework (GPUI statt Raylib)](docs/adr/ADR-0001-ui-framework.md)
- [ADR-0002: Modellanbindung (OpenAI-kompatibel, OpenRouter, lokal)](docs/adr/ADR-0002-modellanbindung.md)
