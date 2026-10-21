//! Voice activity detection and cutting speech into sections of 0.5–25 s (SPEC §5 F2 step 2).

/// Frame size: 32 ms at 16 kHz (Silero's native frame).
pub const FRAME: usize = 512;

/// Speech probability per 512-sample frame.
pub trait VoiceDetector: Send {
    fn speech_prob(&mut self, frame: &[f32]) -> f32;
}

/// Adaptive energy detector. Works without a model; used until Silero is downloaded and in tests.
pub struct EnergyVad {
    noise: f32,
    frames: usize,
}

impl Default for EnergyVad {
    fn default() -> Self {
        EnergyVad { noise: 0.0, frames: 0 }
    }
}

/// Below about -50 dBFS nothing counts as speech.
const ABS_FLOOR: f32 = 0.003;

impl VoiceDetector for EnergyVad {
    fn speech_prob(&mut self, frame: &[f32]) -> f32 {
        let rms = (frame.iter().map(|x| x * x).sum::<f32>() / frame.len().max(1) as f32).sqrt();
        self.frames += 1;
        // Warm-up: the first ~150 ms set the noise floor.
        if self.frames <= 5 {
            self.noise = if self.frames == 1 { rms } else { self.noise.min(rms).max(rms * 0.5) };
            return if rms > ABS_FLOOR * 10.0 { 0.6 } else { 0.0 };
        }
        if rms < ABS_FLOOR {
            self.noise = 0.9 * self.noise + 0.1 * rms;
            return 0.0;
        }
        // Track the noise floor slowly upwards, quickly downwards.
        if rms < self.noise {
            self.noise = 0.9 * self.noise + 0.1 * rms;
        } else {
            self.noise = 0.999 * self.noise + 0.001 * rms;
        }
        let noise = self.noise.max(1e-4);
        let snr_db = 20.0 * (rms / noise).max(1e-6).log10();
        // 6 dB → 0.2, 15 dB → 0.8.
        ((snr_db - 3.0) / 15.0).clamp(0.0, 1.0)
    }
}

/// Silero VAD through whisper.cpp. Scores frames in blocks to keep calls cheap.
#[cfg(feature = "whisper")]
pub struct SileroVad {
    ctx: whisper_rs::WhisperVadContext,
}

#[cfg(feature = "whisper")]
// The context is used from one thread at a time (the pipeline owns it).
unsafe impl Send for SileroVad {}

#[cfg(feature = "whisper")]
impl SileroVad {
    pub fn new(model_path: &std::path::Path) -> crate::Result<Self> {
        let mut p = whisper_rs::WhisperVadContextParams::new();
        p.set_n_threads(1);
        p.set_use_gpu(false);
        let ctx = whisper_rs::WhisperVadContext::new(&model_path.to_string_lossy(), p).map_err(|e| crate::Error::Model(e.to_string()))?;
        Ok(SileroVad { ctx })
    }
}

#[cfg(feature = "whisper")]
impl VoiceDetector for SileroVad {
    fn speech_prob(&mut self, frame: &[f32]) -> f32 {
        if self.ctx.detect_speech(frame).is_err() {
            return 0.0;
        }
        self.ctx.probabilities().iter().copied().fold(0.0, f32::max)
    }
}

#[derive(Debug, Clone)]
pub struct SegmenterConfig {
    pub start_threshold: f32,
    pub end_threshold: f32,
    /// Frames above the start threshold needed to open a section.
    pub start_frames: usize,
    /// Silence that closes a section.
    pub min_silence_ms: u32,
    pub min_speech_ms: u32,
    pub max_speech_ms: u32,
    pub pad_ms: u32,
}

impl Default for SegmenterConfig {
    fn default() -> Self {
        SegmenterConfig {
            start_threshold: 0.5,
            end_threshold: 0.35,
            start_frames: 3,
            min_silence_ms: 550,
            min_speech_ms: 500,
            max_speech_ms: 25_000,
            pad_ms: 200,
        }
    }
}

/// A finished speech section: absolute sample offsets at 16 kHz.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub start: u64,
    pub samples: Vec<f32>,
}

/// Hysteresis segmenter. Feed 16 kHz audio of any length; get finished sections back.
pub struct Segmenter {
    cfg: SegmenterConfig,
    vad: Box<dyn VoiceDetector>,
    buf: Vec<f32>,
    /// Absolute sample index of `buf[0]`.
    buf_start: u64,
    /// Frames not yet scored.
    frame_acc: Vec<f32>,
    processed: u64,
    in_speech: bool,
    speech_start: u64,
    above: usize,
    silence_frames: usize,
    frame_probs: Vec<(u64, f32)>,
}

const MS: u64 = 16;

impl Segmenter {
    pub fn new(vad: Box<dyn VoiceDetector>, cfg: SegmenterConfig) -> Self {
        Segmenter {
            cfg,
            vad,
            buf: Vec::new(),
            buf_start: 0,
            frame_acc: Vec::new(),
            processed: 0,
            in_speech: false,
            speech_start: 0,
            above: 0,
            silence_frames: 0,
            frame_probs: Vec::new(),
        }
    }

    pub fn is_speaking(&self) -> bool {
        self.in_speech
    }

    pub fn push(&mut self, samples: &[f32]) -> Vec<Section> {
        self.buf.extend_from_slice(samples);
        self.frame_acc.extend_from_slice(samples);
        let mut out = Vec::new();
        while self.frame_acc.len() >= FRAME {
            let frame: Vec<f32> = self.frame_acc.drain(..FRAME).collect();
            let p = self.vad.speech_prob(&frame);
            let frame_start = self.processed;
            self.processed += FRAME as u64;
            if let Some(s) = self.step(frame_start, p, &frame) {
                out.push(s);
            }
        }
        self.trim();
        out
    }

    /// Closes an open section (end of recording).
    pub fn flush(&mut self) -> Option<Section> {
        if self.in_speech {
            self.in_speech = false;
            return self.cut(self.speech_start, self.processed);
        }
        None
    }

    fn step(&mut self, frame_start: u64, p: f32, frame: &[f32]) -> Option<Section> {
        let silence_needed = (self.cfg.min_silence_ms as u64 * MS).div_ceil(FRAME as u64) as usize;
        if !self.in_speech {
            if p >= self.cfg.start_threshold {
                self.above += 1;
                if self.above >= self.cfg.start_frames {
                    self.in_speech = true;
                    self.silence_frames = 0;
                    self.frame_probs.clear();
                    let back = (self.above as u64 - 1) * FRAME as u64;
                    self.speech_start = frame_start.saturating_sub(back);
                }
            } else {
                self.above = 0;
            }
            return None;
        }
        let energy = frame.iter().map(|x| x * x).sum::<f32>();
        self.frame_probs.push((frame_start, energy));
        if p < self.cfg.end_threshold {
            self.silence_frames += 1;
        } else {
            self.silence_frames = 0;
        }
        let end = frame_start + FRAME as u64;
        if self.silence_frames >= silence_needed {
            self.in_speech = false;
            self.above = 0;
            let speech_end = end - (self.silence_frames as u64 * FRAME as u64);
            return self.cut(self.speech_start, speech_end);
        }
        if end - self.speech_start >= self.cfg.max_speech_ms as u64 * MS {
            // Too long: cut at the quietest frame of the last 3 s.
            let window_start = end.saturating_sub(3_000 * MS);
            let cut_at = self
                .frame_probs
                .iter()
                .filter(|(s, _)| *s >= window_start)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(s, _)| *s + FRAME as u64 / 2)
                .unwrap_or(end);
            let sec = self.cut(self.speech_start, cut_at);
            self.speech_start = cut_at;
            self.frame_probs.retain(|(s, _)| *s >= cut_at);
            return sec;
        }
        None
    }

    fn cut(&mut self, start: u64, end: u64) -> Option<Section> {
        if end.saturating_sub(start) < self.cfg.min_speech_ms as u64 * MS {
            return None;
        }
        let pad = self.cfg.pad_ms as u64 * MS;
        let s = start.saturating_sub(pad).max(self.buf_start);
        let e = (end + pad).min(self.buf_start + self.buf.len() as u64);
        let a = (s - self.buf_start) as usize;
        let b = (e - self.buf_start) as usize;
        Some(Section { start: s, samples: self.buf[a..b].to_vec() })
    }

    /// Keeps only what an open section (or padding) may still need.
    fn trim(&mut self) {
        let keep_from = if self.in_speech { self.speech_start } else { self.processed };
        let keep_from = keep_from.saturating_sub(self.cfg.pad_ms as u64 * MS * 2);
        if keep_from > self.buf_start {
            let n = ((keep_from - self.buf_start) as usize).min(self.buf.len());
            self.buf.drain(..n);
            self.buf_start += n as u64;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(ms: u32, amp: f32) -> Vec<f32> {
        (0..(ms as usize * 16)).map(|i| amp * (i as f32 * 0.2).sin()).collect()
    }

    #[test]
    fn finds_two_utterances() {
        let mut seg = Segmenter::new(Box::new(EnergyVad::default()), SegmenterConfig::default());
        let mut audio = tone(1000, 0.0005);
        audio.extend(tone(1500, 0.3));
        audio.extend(tone(1000, 0.0005));
        audio.extend(tone(2000, 0.3));
        audio.extend(tone(1000, 0.0005));
        let mut secs = Vec::new();
        for c in audio.chunks(700) {
            secs.extend(seg.push(c));
        }
        secs.extend(seg.flush());
        assert_eq!(secs.len(), 2, "{:?}", secs.iter().map(|s| (s.start, s.samples.len())).collect::<Vec<_>>());
        let first_ms = secs[0].start / 16;
        assert!((700..=1100).contains(&first_ms), "start {first_ms}");
        let len_ms = secs[1].samples.len() as u64 / 16;
        assert!((1900..=2600).contains(&len_ms), "len {len_ms}");
    }

    #[test]
    fn cuts_long_speech() {
        let mut seg = Segmenter::new(Box::new(EnergyVad::default()), SegmenterConfig { max_speech_ms: 5_000, ..Default::default() });
        let mut audio = tone(500, 0.0005);
        audio.extend(tone(12_000, 0.3));
        let mut secs = Vec::new();
        for c in audio.chunks(1600) {
            secs.extend(seg.push(c));
        }
        secs.extend(seg.flush());
        assert!(secs.len() >= 3);
        assert!(secs.iter().all(|s| s.samples.len() <= (5_000 + 400) * 16));
    }

    #[test]
    fn ignores_clicks() {
        let mut seg = Segmenter::new(Box::new(EnergyVad::default()), SegmenterConfig::default());
        let mut audio = tone(1000, 0.0005);
        audio.extend(tone(100, 0.5));
        audio.extend(tone(1500, 0.0005));
        let secs: Vec<_> = audio.chunks(512).flat_map(|c| seg.push(c)).collect();
        assert!(secs.is_empty());
    }
}
