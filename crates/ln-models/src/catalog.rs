//! Sizes and SHA-256 from the Hugging Face API, checked 2026-10-21.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    Asr,
    Vad,
    Embedding,
    Llm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelFile {
    pub name: &'static str,
    pub url: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelSpec {
    pub id: &'static str,
    pub display: &'static str,
    pub kind: ModelKind,
    pub license: &'static str,
    pub license_url: &'static str,
    /// Must the user accept terms before download (Gemma Terms of Use)?
    pub needs_terms: bool,
    pub files: &'static [ModelFile],
    /// Approximate RAM while loaded.
    pub ram_mb: u32,
    pub min_ram_gb: u32,
}

impl ModelSpec {
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

pub const CATALOG: &[ModelSpec] = &[
    ModelSpec {
        id: "whisper-large-v3-turbo-q5_0",
        display: "Whisper large-v3-turbo",
        kind: ModelKind::Asr,
        license: "MIT",
        license_url: "https://github.com/openai/whisper/blob/main/LICENSE",
        needs_terms: false,
        files: &[ModelFile {
            name: "ggml-large-v3-turbo-q5_0.bin",
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
            size: 574_041_195,
            sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        }],
        ram_mb: 1000,
        min_ram_gb: 4,
    },
    ModelSpec {
        id: "whisper-small-q8_0",
        display: "Whisper small",
        kind: ModelKind::Asr,
        license: "MIT",
        license_url: "https://github.com/openai/whisper/blob/main/LICENSE",
        needs_terms: false,
        files: &[ModelFile {
            name: "ggml-small-q8_0.bin",
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q8_0.bin",
            size: 264_464_607,
            sha256: "49c8fb02b65e6049d5fa6c04f81f53b867b5ec9540406812c643f177317f779f",
        }],
        ram_mb: 450,
        min_ram_gb: 4,
    },
    ModelSpec {
        id: "whisper-base-q8_0",
        display: "Whisper base (Spracherkennung)",
        kind: ModelKind::Asr,
        license: "MIT",
        license_url: "https://github.com/openai/whisper/blob/main/LICENSE",
        needs_terms: false,
        files: &[ModelFile {
            name: "ggml-base-q8_0.bin",
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base-q8_0.bin",
            size: 81_768_585,
            sha256: "c577b9a86e7e048a0b7eada054f4dd79a56bbfa911fbdacf900ac5b567cbb7d9",
        }],
        ram_mb: 150,
        min_ram_gb: 2,
    },
    ModelSpec {
        id: "silero-vad-v5",
        display: "Silero VAD",
        kind: ModelKind::Vad,
        license: "MIT",
        license_url: "https://github.com/snakers4/silero-vad/blob/master/LICENSE",
        needs_terms: false,
        files: &[ModelFile {
            name: "ggml-silero-v5.1.2.bin",
            url: "https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin",
            size: 885_098,
            sha256: "29940d98d42b91fbd05ce489f3ecf7c72f0a42f027e4875919a28fb4c04ea2cf",
        }],
        ram_mb: 10,
        min_ram_gb: 1,
    },
    ModelSpec {
        id: "embeddinggemma-300m",
        display: "EmbeddingGemma 300M",
        kind: ModelKind::Embedding,
        license: "Gemma Terms of Use",
        license_url: "https://ai.google.dev/gemma/terms",
        needs_terms: true,
        files: &[ModelFile {
            name: "embeddinggemma-300M-Q8_0.gguf",
            url: "https://huggingface.co/ggml-org/embeddinggemma-300M-GGUF/resolve/main/embeddinggemma-300M-Q8_0.gguf",
            size: 333_590_944,
            sha256: "b5ce9d77a3fc4b3b39ccb5643c36777911cc4eb46a66962eadfa3f5f60490d63",
        }],
        ram_mb: 450,
        min_ram_gb: 2,
    },
    ModelSpec {
        id: "gemma-4-e2b-it",
        display: "Gemma 4 E2B",
        kind: ModelKind::Llm,
        license: "Apache-2.0",
        license_url: "https://www.apache.org/licenses/LICENSE-2.0",
        needs_terms: false,
        files: &[
            ModelFile {
                name: "gemma-4-E2B-it-Q4_K_M.gguf",
                url: "https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/gemma-4-E2B-it-Q4_K_M.gguf",
                size: 3_106_738_272,
                sha256: "740185b21d22ceb83a11c3aa62ad5842ef32c70f6096d756bbee85a1e4ec34b8",
            },
            ModelFile {
                name: "mmproj-gemma-4-E2B-it-Q8_0.gguf",
                url: "https://huggingface.co/ggml-org/gemma-4-E2B-it-GGUF/resolve/main/mmproj-gemma-4-E2B-it-Q8_0.gguf",
                size: 557_368_064,
                sha256: "9406f99c16d68cda4f1f0552192dcc99021ea1fc6d2fd50b1dc3ccf30d04b292",
            },
        ],
        ram_mb: 3800,
        min_ram_gb: 6,
    },
    ModelSpec {
        id: "gemma-4-e4b-it",
        display: "Gemma 4 E4B",
        kind: ModelKind::Llm,
        license: "Apache-2.0",
        license_url: "https://www.apache.org/licenses/LICENSE-2.0",
        needs_terms: false,
        files: &[
            ModelFile {
                name: "gemma-4-E4B-it-Q4_K_M.gguf",
                url: "https://huggingface.co/unsloth/gemma-4-E4B-it-GGUF/resolve/main/gemma-4-E4B-it-Q4_K_M.gguf",
                size: 4_977_171_584,
                sha256: "85a896a047553e842f25297ee5b031d64ff30147d9c4af17b1e4b394cd1fab87",
            },
            ModelFile {
                name: "mmproj-gemma-4-E4B-it-Q8_0.gguf",
                url: "https://huggingface.co/ggml-org/gemma-4-E4B-it-GGUF/resolve/main/mmproj-gemma-4-E4B-it-Q8_0.gguf",
                size: 559_874_816,
                sha256: "197f49a93027f9843772bd24a6a9e0be2a32a788de5a3def330e9c585d86edd1",
            },
        ],
        ram_mb: 6000,
        min_ram_gb: 15,
    },
];

pub fn by_id(id: &str) -> Option<&'static ModelSpec> {
    CATALOG.iter().find(|m| m.id == id)
}

/// Like Foresight: < 15 GB → E2B, otherwise E4B (12B is not offered locally in v1.0).
pub fn recommended_llm(ram_gb: u32) -> &'static ModelSpec {
    if ram_gb >= 15 { by_id("gemma-4-e4b-it").unwrap() } else { by_id("gemma-4-e2b-it").unwrap() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_is_consistent() {
        for m in CATALOG {
            for f in m.files {
                assert_eq!(f.sha256.len(), 64, "{}", f.name);
                assert!(f.url.ends_with(f.name), "{}", f.name);
            }
        }
        assert_eq!(recommended_llm(8).id, "gemma-4-e2b-it");
        assert_eq!(recommended_llm(16).id, "gemma-4-e4b-it");
        assert!(by_id("embeddinggemma-300m").unwrap().needs_terms);
    }
}
