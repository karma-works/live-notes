use crate::{Error, Result};
use ln_types::Lang;

/// A speech-to-text backend. Implementations: local Whisper and cloud (OpenAI-compatible).
pub trait Transcriber: Send {
    /// Written into the note's front matter, e.g. `local/whisper-large-v3-turbo`.
    fn name(&self) -> String;
    /// True if the backend reports the language itself (cloud); then `detect` is not used.
    fn detects_language(&self) -> bool {
        false
    }
    /// Language probabilities for a section (16 kHz mono).
    fn detect(&mut self, pcm: &[f32]) -> Result<Vec<(Lang, f32)>>;
    /// Transcribes in `lang` (never translates). `hints` are the user's languages, `prompt`
    /// recent text in the same language. Returns the text and the language the backend reports.
    fn transcribe(&mut self, pcm: &[f32], lang: Option<&Lang>, hints: &[Lang], prompt: &str) -> Result<(String, Option<Lang>)>;
}

#[cfg(feature = "whisper")]
pub use whisper::WhisperTranscriber;

#[cfg(feature = "whisper")]
mod whisper {
    use super::*;
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

    pub struct WhisperTranscriber {
        _ctx: WhisperContext,
        state: WhisperState,
        /// Small model for language detection (whisper base): its encoder is ~20x cheaper.
        lid: Option<(WhisperContext, WhisperState)>,
        model_name: String,
        threads: usize,
    }

    fn load_ctx(path: &std::path::Path) -> Result<(WhisperContext, WhisperState)> {
        let mut p = WhisperContextParameters::default();
        p.use_gpu(std::env::var("LN_WHISPER_CPU").is_err());
        p.flash_attn(true);
        let ctx = WhisperContext::new_with_params(path, p).map_err(|e| Error::Model(e.to_string()))?;
        let state = ctx.create_state().map_err(|e| Error::Model(e.to_string()))?;
        Ok((ctx, state))
    }

    impl WhisperTranscriber {
        pub fn load(path: &std::path::Path, model_name: &str) -> Result<Self> {
            Self::load_with_lid(path, model_name, None)
        }

        /// `lid` is an optional small Whisper model used only for language detection.
        pub fn load_with_lid(path: &std::path::Path, model_name: &str, lid: Option<&std::path::Path>) -> Result<Self> {
            let (ctx, state) = load_ctx(path)?;
            let lid = match lid {
                Some(p) => Some(load_ctx(p)?),
                None => None,
            };
            let threads = perf_threads();
            Ok(WhisperTranscriber { _ctx: ctx, state, lid, model_name: model_name.into(), threads })
        }
    }

    /// Threads for Whisper: the performance cores minus one, so the UI thread always gets a core
    /// (all 8 cores of an M1 froze the window while sections were transcribed).
    fn perf_threads() -> usize {
        let perf = std::process::Command::new("sysctl")
            .args(["-n", "hw.perflevel0.physicalcpu"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse::<usize>().ok());
        let all = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        perf.unwrap_or(all / 2).saturating_sub(1).clamp(2, 8)
    }

    /// Whisper needs at least one second of audio.
    fn padded(pcm: &[f32]) -> std::borrow::Cow<'_, [f32]> {
        let min = crate::RATE as usize + 1600;
        if pcm.len() >= min {
            std::borrow::Cow::Borrowed(pcm)
        } else {
            let mut v = pcm.to_vec();
            v.resize(min, 0.0);
            std::borrow::Cow::Owned(v)
        }
    }

    impl Transcriber for WhisperTranscriber {
        fn name(&self) -> String {
            format!("local/{}", self.model_name)
        }

        fn detect(&mut self, pcm: &[f32]) -> Result<Vec<(Lang, f32)>> {
            let pcm = padded(pcm);
            let threads = self.threads;
            let state = match self.lid.as_mut() {
                Some((_, st)) => st,
                None => &mut self.state,
            };
            state.pcm_to_mel(&pcm, threads).map_err(|e| Error::Transcribe(e.to_string()))?;
            let (_, probs) = state.lang_detect(0, threads).map_err(|e| Error::Transcribe(e.to_string()))?;
            Ok(probs.iter().enumerate().filter_map(|(i, p)| whisper_rs::get_lang_str(i as i32).map(|s| (Lang::new(s), *p))).collect())
        }

        fn transcribe(&mut self, pcm: &[f32], lang: Option<&Lang>, _hints: &[Lang], prompt: &str) -> Result<(String, Option<Lang>)> {
            let pcm = padded(pcm);
            let code = lang.map(|l| l.as_str().to_string());
            let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            p.set_n_threads(self.threads as i32);
            p.set_language(code.as_deref());
            p.set_translate(false);
            p.set_no_context(true);
            p.set_no_timestamps(true);
            p.set_print_special(false);
            p.set_print_progress(false);
            p.set_print_realtime(false);
            p.set_print_timestamps(false);
            p.set_suppress_blank(true);
            p.set_suppress_nst(true);
            p.set_no_speech_thold(0.6);
            p.set_temperature(0.0);
            // Full 30 s encoder window (the default). A window shortened to the section length was
            // 3–4× faster, but made short sections loop or turn into gibberish (a 2.4 s Chinese
            // sentence became mixed-script noise) and cost accuracy on Chinese; see
            // target/test-corpus/hard and docs/STATUS.md.
            if !prompt.is_empty() {
                p.set_initial_prompt(prompt);
            }
            self.state.full(p, &pcm).map_err(|e| Error::Transcribe(e.to_string()))?;
            let mut text = String::new();
            for i in 0..self.state.full_n_segments() {
                if let Some(seg) = self.state.get_segment(i) {
                    if seg.no_speech_probability() > 0.6 {
                        continue;
                    }
                    if let Ok(t) = seg.to_str_lossy() {
                        text.push_str(&t);
                    }
                }
            }
            Ok((text.trim().to_string(), lang.cloned()))
        }
    }
}

/// Cloud transcription over the OpenAI-compatible `/audio/transcriptions` endpoint
/// (`gpt-transcribe`, SPEC §5 F2 mode "OpenAI günstig"). Blocks on the given runtime.
pub struct CloudTranscriber {
    client: ln_ai::Client,
    model: String,
    rt: tokio::runtime::Handle,
    /// Seconds of audio sent, for cost tracking.
    pub seconds_sent: f64,
}

impl CloudTranscriber {
    pub fn new(client: ln_ai::Client, model: impl Into<String>, rt: tokio::runtime::Handle) -> Self {
        CloudTranscriber { client, model: model.into(), rt, seconds_sent: 0.0 }
    }
}

impl Transcriber for CloudTranscriber {
    fn name(&self) -> String {
        format!("{}/{}", self.client.provider().id, self.model)
    }
    fn detects_language(&self) -> bool {
        true
    }
    fn detect(&mut self, _pcm: &[f32]) -> Result<Vec<(Lang, f32)>> {
        Ok(vec![])
    }
    fn transcribe(&mut self, pcm: &[f32], lang: Option<&Lang>, hints: &[Lang], prompt: &str) -> Result<(String, Option<Lang>)> {
        let wav = crate::wav::encode(pcm, crate::RATE);
        let langs: Vec<String> = match lang {
            Some(l) => vec![l.to_string()],
            None => hints.iter().map(|l| l.to_string()).collect(),
        };
        let prompt = (!prompt.is_empty()).then_some(prompt);
        let client = self.client.clone();
        let model = self.model.clone();
        let t = self.rt.block_on(async move { client.transcribe(&model, wav, &langs, prompt).await })?;
        self.seconds_sent += pcm.len() as f64 / crate::RATE as f64;
        let reported = t.languages.first().map(Lang::new).or_else(|| lang.cloned());
        Ok((t.text, reported))
    }
}
