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

    /// Forgets language and prompt context: the next section belongs to another recording.
    pub fn reset(&mut self) {
        self.previous.clear();
        self.context.clear();
        self.forced_language = None;
    }

    pub fn transcriber_name(&self) -> String {
        self.transcriber.name()
    }

    pub fn transcriber_mut(&mut self) -> &mut dyn Transcriber {
        self.transcriber.as_mut()
    }

    /// Transcribes one section. `override_lang` is set when the user corrected the language.
    pub fn process(&mut self, seg: &PendingSegment, override_lang: Option<&Lang>) -> Result<Option<Segment>> {
        let (lang, conf) = self.language(seg, override_lang)?;
        self.transcribe_as(seg, lang, conf)
    }

    /// Transcribes sections that were waiting in a queue. Consecutive sections of the same channel
    /// and language share one Whisper run: the encoder always processes a 30 s window, so a
    /// 2 s section costs as much as a 28 s one. Packing cuts the runs of a typical conversation
    /// by about a quarter (more for monologues). Each result belongs to the returned section;
    /// a packed section spans its parts.
    pub fn process_packed(&mut self, segs: Vec<PendingSegment>) -> Vec<(PendingSegment, Result<Option<Segment>>)> {
        if segs.len() < 2 || self.transcriber.detects_language() {
            return segs
                .into_iter()
                .map(|s| {
                    let r = self.process(&s, None);
                    (s, r)
                })
                .collect();
        }
        let mut out = Vec::new();
        let mut group: Vec<PendingSegment> = Vec::new();
        let mut group_lang: Option<(Lang, f32)> = None;
        for s in segs {
            let (lang, conf) = match self.language(&s, None) {
                Ok((Some(l), c)) => (l, c),
                Ok((None, _)) => unreachable!("only backends that detect the language return none"),
                Err(e) => {
                    self.flush_group(&mut group, group_lang.take(), &mut out);
                    out.push((s, Err(e)));
                    continue;
                }
            };
            // The next section's language choice leans on this one, as it would one by one.
            self.previous.insert(s.channel, lang.clone());
            let fits = group.first().is_some_and(|g| g.channel == s.channel)
                && group_lang.as_ref().is_some_and(|(l, _)| *l == lang)
                && packed_len(&group) + PACK_GAP + s.pcm.len() <= PACK_SAMPLES;
            if !fits {
                self.flush_group(&mut group, group_lang.take(), &mut out);
            }
            group_lang = Some(match group_lang {
                Some((l, c)) => (l, c.min(conf)),
                None => (lang, conf),
            });
            group.push(s);
        }
        self.flush_group(&mut group, group_lang, &mut out);
        out
    }

    fn flush_group(
        &mut self,
        group: &mut Vec<PendingSegment>,
        lang: Option<(Lang, f32)>,
        out: &mut Vec<(PendingSegment, Result<Option<Segment>>)>,
    ) {
        if group.is_empty() {
            return;
        }
        let seg = pack(std::mem::take(group));
        let (lang, conf) = lang.map(|(l, c)| (Some(l), c)).unwrap_or((None, 0.0));
        let r = self.transcribe_as(&seg, lang, conf);
        out.push((seg, r));
    }

    /// The language to transcribe a section in, with its confidence.
    fn language(&mut self, seg: &PendingSegment, override_lang: Option<&Lang>) -> Result<(Option<Lang>, f32)> {
        let duration = seg.end_ms.saturating_sub(seg.start_ms);
        let fixed = override_lang.cloned().or_else(|| self.forced_language.clone());
        Ok(match fixed {
            Some(l) => (Some(l), 1.0),
            None if self.transcriber.detects_language() => (None, 0.0),
            None => {
                let probs = self.transcriber.detect(&seg.pcm)?;
                let (l, c) = choose_language(&probs, &self.allowed, self.previous.get(&seg.channel), duration);
                (Some(l), c)
            }
        })
    }

    fn transcribe_as(&mut self, seg: &PendingSegment, lang: Option<Lang>, conf: f32) -> Result<Option<Segment>> {
        let duration = seg.end_ms.saturating_sub(seg.start_ms);
        let prompt = lang.as_ref().and_then(|l| self.context.get(&(seg.channel, l.clone()))).cloned().unwrap_or_default();
        let (text, reported) = self.transcriber.transcribe(&seg.pcm, lang.as_ref(), &self.allowed, &prompt)?;
        let lang = reported
            .filter(|l| self.allowed.is_empty() || self.allowed.contains(l))
            .or(lang)
            .or_else(|| self.previous.get(&seg.channel).cloned())
            .unwrap_or_else(|| self.allowed.first().cloned().unwrap_or_else(|| Lang::new("en")));
        let conf = if conf == 0.0 { 1.0 } else { conf };
        let Some(text) = self.post.process(&text, &lang) else { return Ok(None) };
        if !crate::post::plausible(&text, duration) {
            tracing::warn!("dropped implausible transcript: {} characters for {} ms of audio", text.chars().count(), duration);
            return Ok(None);
        }
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

/// Most audio one packed Whisper run holds (its window is 30 s).
const PACK_SAMPLES: usize = 28 * crate::RATE as usize;
/// Silence between packed sections, so words do not run together.
const PACK_GAP: usize = crate::RATE as usize * 3 / 10;

fn packed_len(group: &[PendingSegment]) -> usize {
    group.iter().map(|s| s.pcm.len()).sum::<usize>() + PACK_GAP * group.len().saturating_sub(1)
}

/// Joins sections of one channel into one, with a short silence between them.
fn pack(mut group: Vec<PendingSegment>) -> PendingSegment {
    if group.len() == 1 {
        return group.pop().unwrap();
    }
    let mut pcm = Vec::with_capacity(packed_len(&group));
    for (i, s) in group.iter().enumerate() {
        if i > 0 {
            pcm.resize(pcm.len() + PACK_GAP, 0.0);
        }
        pcm.extend_from_slice(&s.pcm);
    }
    PendingSegment { channel: group[0].channel, start_ms: group[0].start_ms, end_ms: group.last().unwrap().end_ms, pcm: Arc::new(pcm) }
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
            Ok(if peak > 0.4 {
                vec![(Lang::new("zh"), 0.9), (Lang::new("ja"), 0.1)]
            } else {
                vec![(Lang::new("de"), 0.7), (Lang::new("nl"), 0.3)]
            })
        }
        fn transcribe(&mut self, _pcm: &[f32], lang: Option<&Lang>, _h: &[Lang], prompt: &str) -> Result<(String, Option<Lang>)> {
            let l = lang.unwrap().clone();
            let t = if l.as_str() == "zh" { "我們開會." } else { "Hallo zusammen" };
            assert!(prompt.is_empty() || !prompt.contains(if l.as_str() == "zh" { "Hallo" } else { "开会" }), "prompt mixes languages");
            Ok((t.to_string(), Some(l)))
        }
    }

    /// Counts Whisper runs; loud sections are Chinese, quiet ones German.
    struct CountingAsr(Arc<std::sync::atomic::AtomicUsize>);
    impl Transcriber for CountingAsr {
        fn name(&self) -> String {
            "count".into()
        }
        fn detect(&mut self, pcm: &[f32]) -> Result<Vec<(Lang, f32)>> {
            FakeAsr.detect(pcm)
        }
        fn transcribe(&mut self, pcm: &[f32], lang: Option<&Lang>, _h: &[Lang], _p: &str) -> Result<(String, Option<Lang>)> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let words = if lang.unwrap().as_str() == "zh" { "好" } else { "ja" };
            Ok((format!("{words} {}", pcm.len() / 16_000), lang.cloned()))
        }
    }

    fn section(channel: Channel, start_ms: u64, ms: u64, amp: f32) -> PendingSegment {
        let pcm = (0..(ms as usize * 16)).map(|i| amp * (i as f32 * 0.07).sin()).collect();
        PendingSegment { channel, start_ms, end_ms: start_ms + ms, pcm: Arc::new(pcm) }
    }

    #[test]
    fn packs_consecutive_sections_of_one_channel_and_language() {
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut engine =
            AsrEngine::new(Box::new(CountingAsr(runs.clone())), PostProcessor::new(ChineseScript::Simplified), Lang::main_languages());
        let segs = vec![
            section(Channel::Me, 0, 3000, 0.2),        // German
            section(Channel::Me, 4000, 2000, 0.2),     // German: packed with the first
            section(Channel::Me, 7000, 2000, 0.6),     // Chinese: new run
            section(Channel::Others, 9500, 2000, 0.6), // other channel: new run
            section(Channel::Others, 12000, 20000, 0.6),
            section(Channel::Others, 33000, 10000, 0.6), // would exceed 28 s: new run
        ];
        let out = engine.process_packed(segs);
        assert_eq!(runs.load(std::sync::atomic::Ordering::Relaxed), 4);
        let spans: Vec<(u64, u64)> = out.iter().map(|(p, _)| (p.start_ms, p.end_ms)).collect();
        assert_eq!(spans, vec![(0, 6000), (7000, 9000), (9500, 32000), (33000, 43000)]);
        let first = out[0].1.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(first.lang.as_str(), "de");
        assert_eq!(first.text, "ja 5", "3 s + 0.3 s gap + 2 s of audio in one run");
        assert_eq!(out[0].0.pcm.len(), (3000 + 300 + 2000) * 16);
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
