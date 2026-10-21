use crate::vad::{Segmenter, SegmenterConfig, VoiceDetector};
use crate::{PostProcessor, Result, StreamResampler, Transcriber, choose_language};
use ln_types::{Channel, Lang, Segment};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// A speech section waiting for transcription. Audio stays in RAM only (SPEC §5 F1).
#[derive(Debug, Clone)]
pub struct PendingSegment {
    pub channel: Channel,
    pub start_ms: u64,
    pub end_ms: u64,
    pub pcm: Arc<Vec<f32>>,
}

/// Resampler + VAD segmenter for one channel.
pub struct ChannelSegmenter {
    pub channel: Channel,
    resampler: Option<StreamResampler>,
    segmenter: Segmenter,
}

impl ChannelSegmenter {
    pub fn new(channel: Channel, vad: Box<dyn VoiceDetector>) -> Self {
        ChannelSegmenter { channel, resampler: None, segmenter: Segmenter::new(vad, SegmenterConfig::default()) }
    }

    pub fn is_speaking(&self) -> bool {
        self.segmenter.is_speaking()
    }

    /// Feeds device audio at `rate`; returns finished sections.
    pub fn push(&mut self, samples: &[f32], rate: u32) -> Vec<PendingSegment> {
        if self.resampler.as_ref().map(|r| r.rate_in()) != Some(rate) {
            self.resampler = Some(StreamResampler::new(rate));
        }
        let pcm = self.resampler.as_mut().unwrap().push(samples);
        self.segmenter.push(&pcm).into_iter().map(|s| self.pending(s)).collect()
    }

    pub fn flush(&mut self) -> Option<PendingSegment> {
        self.segmenter.flush().map(|s| self.pending(s))
    }

    fn pending(&self, s: crate::vad::Section) -> PendingSegment {
        let start_ms = s.start / 16;
        let end_ms = start_ms + s.samples.len() as u64 / 16;
        PendingSegment { channel: self.channel, start_ms, end_ms, pcm: Arc::new(s.samples) }
    }
}

/// Last 10 minutes of sections, for re-transcription after a language correction.
#[derive(Default)]
pub struct AudioRing {
    items: VecDeque<(usize, PendingSegment)>,
}

impl AudioRing {
    pub const KEEP_MS: u64 = 10 * 60 * 1000;

    pub fn push(&mut self, index: usize, seg: PendingSegment) {
        let now = seg.end_ms;
        self.items.push_back((index, seg));
        while self.items.front().is_some_and(|(_, s)| now.saturating_sub(s.end_ms) > Self::KEEP_MS) {
            self.items.pop_front();
        }
    }

    pub fn get(&self, index: usize) -> Option<&PendingSegment> {
        self.items.iter().find(|(i, _)| *i == index).map(|(_, s)| s)
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }
}

/// Turns sections into transcript segments: language choice, transcription, post-processing.
pub struct AsrEngine {
    transcriber: Box<dyn Transcriber>,
    post: PostProcessor,
    allowed: Vec<Lang>,
    /// "The whole recording is Chinese".
    pub forced_language: Option<Lang>,
    previous: HashMap<Channel, Lang>,
    /// Recent text per channel and language, used as prompt (SPEC §5 F2 step 6).
    context: HashMap<(Channel, Lang), String>,
}

impl AsrEngine {
    pub fn new(transcriber: Box<dyn Transcriber>, post: PostProcessor, allowed: Vec<Lang>) -> Self {
        AsrEngine { transcriber, post, allowed, forced_language: None, previous: HashMap::new(), context: HashMap::new() }
    }

    pub fn transcriber_name(&self) -> String {
        self.transcriber.name()
    }

    pub fn transcriber_mut(&mut self) -> &mut dyn Transcriber {
        self.transcriber.as_mut()
    }

    /// Transcribes one section. `override_lang` is set when the user corrected the language.
    pub fn process(&mut self, seg: &PendingSegment, override_lang: Option<&Lang>) -> Result<Option<Segment>> {
        let duration = seg.end_ms.saturating_sub(seg.start_ms);
        let fixed = override_lang.cloned().or_else(|| self.forced_language.clone());
        let (lang, conf) = match fixed {
            Some(l) => (Some(l), 1.0),
            None if self.transcriber.detects_language() => (None, 0.0),
            None => {
                let probs = self.transcriber.detect(&seg.pcm)?;
                let (l, c) = choose_language(&probs, &self.allowed, self.previous.get(&seg.channel), duration);
                (Some(l), c)
            }
        };
        let prompt = lang.as_ref().and_then(|l| self.context.get(&(seg.channel, l.clone()))).cloned().unwrap_or_default();
        let (text, reported) = self.transcriber.transcribe(&seg.pcm, lang.as_ref(), &self.allowed, &prompt)?;
        let lang = reported
            .filter(|l| self.allowed.is_empty() || self.allowed.contains(l))
            .or(lang)
            .or_else(|| self.previous.get(&seg.channel).cloned())
            .unwrap_or_else(|| self.allowed.first().cloned().unwrap_or_else(|| Lang::new("en")));
        let conf = if conf == 0.0 { 1.0 } else { conf };
        let Some(text) = self.post.process(&text, &lang) else { return Ok(None) };
        self.previous.insert(seg.channel, lang.clone());
        let ctx = self.context.entry((seg.channel, lang.clone())).or_default();
        ctx.push(' ');
        ctx.push_str(&text);
        if ctx.chars().count() > 220 {
            let skip = ctx.chars().count() - 220;
            *ctx = ctx.chars().skip(skip).collect();
        }
        Ok(Some(Segment { channel: seg.channel, start_ms: seg.start_ms, end_ms: seg.end_ms, lang, text, lang_confidence: conf }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EnergyVad;
    use ln_types::ChineseScript;

    /// Pretends: loud sections are Chinese, quiet ones German; text is the section length.
    struct FakeAsr;
    impl Transcriber for FakeAsr {
        fn name(&self) -> String {
            "fake".into()
        }
        fn detect(&mut self, pcm: &[f32]) -> Result<Vec<(Lang, f32)>> {
            let peak = pcm.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            Ok(if peak > 0.4 { vec![(Lang::new("zh"), 0.9), (Lang::new("ja"), 0.1)] } else { vec![(Lang::new("de"), 0.7), (Lang::new("nl"), 0.3)] })
        }
        fn transcribe(&mut self, pcm: &[f32], lang: Option<&Lang>, _h: &[Lang], prompt: &str) -> Result<(String, Option<Lang>)> {
            let l = lang.unwrap().clone();
            let t = if l.as_str() == "zh" { "我們開會." } else { "Hallo zusammen" };
            assert!(prompt.is_empty() || !prompt.contains(if l.as_str() == "zh" { "Hallo" } else { "开会" }), "prompt mixes languages");
            Ok((format!("{t}{}", if pcm.is_empty() { "" } else { "" }), Some(l)))
        }
    }

    fn tone(ms: u32, amp: f32) -> Vec<f32> {
        (0..(ms as usize * 48)).map(|i| amp * (i as f32 * 0.07).sin()).collect()
    }

    #[test]
    fn end_to_end_with_language_switch() {
        let mut seg = ChannelSegmenter::new(Channel::Others, Box::new(EnergyVad::default()));
        let mut audio = tone(800, 0.0005);
        audio.extend(tone(1500, 0.2)); // "German"
        audio.extend(tone(900, 0.0005));
        audio.extend(tone(1500, 0.6)); // "Chinese"
        audio.extend(tone(900, 0.0005));
        let mut pending = Vec::new();
        for c in audio.chunks(4800) {
            pending.extend(seg.push(c, 48_000));
        }
        pending.extend(seg.flush());
        assert_eq!(pending.len(), 2);
        let mut engine = AsrEngine::new(Box::new(FakeAsr), PostProcessor::new(ChineseScript::Simplified), Lang::main_languages());
        let out: Vec<Segment> = pending.iter().filter_map(|p| engine.process(p, None).unwrap()).collect();
        assert_eq!(out[0].lang.as_str(), "de");
        assert_eq!(out[0].text, "Hallo zusammen");
        assert_eq!(out[1].lang.as_str(), "zh");
        assert_eq!(out[1].text, "我们开会。", "converted to simplified with full-width stop");
        assert!(out[1].start_ms > out[0].end_ms);

        // Correction: force German on the Chinese section.
        let fixed = engine.process(&pending[1], Some(&Lang::new("de"))).unwrap().unwrap();
        assert_eq!(fixed.lang.as_str(), "de");

        let mut ring = AudioRing::default();
        ring.push(0, pending[0].clone());
        assert!(ring.get(0).is_some());
        assert!(ring.get(1).is_none());
    }
}
