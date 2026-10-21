//! Audio self-test: records each channel for a few seconds without UI and reports what arrived
//! (sample rate, level, speech sections, transcript). Used by `live-notes --probe-capture`, by the
//! "Test recording" button and by the verification skill.

use crate::Core;
use ln_asr::{AsrEngine, ChannelSegmenter, PostProcessor, VoiceDetector};
use ln_audio::{Capture, CaptureConfig, Source};
use ln_platform::Permission;
use ln_types::Channel;
use serde::Serialize;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ChannelProbe {
    pub channel: String,
    pub started: bool,
    pub error: Option<String>,
    pub sample_rate: u32,
    pub seconds: f32,
    pub rms: f32,
    pub peak: f32,
    /// Share of samples that are not exactly zero. 0.0 means the OS delivers pure silence,
    /// typically because the permission is missing.
    pub nonzero: f32,
    pub sections: usize,
    /// (language, text) per transcribed section.
    pub transcript: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ProbeReport {
    pub microphone_permission: String,
    pub vad: String,
    pub transcriber: Option<String>,
    pub channels: Vec<ChannelProbe>,
    /// Plain-language findings, most important first.
    pub findings: Vec<String>,
}

fn perm_name(p: Permission) -> &'static str {
    match p {
        Permission::Granted => "granted",
        Permission::Denied => "denied",
        Permission::NotDetermined => "not_determined",
    }
}

impl Core {
    /// Records both channels for `seconds`, then segments and (optionally) transcribes them.
    pub fn probe_capture(&self, seconds: u64, transcribe: bool) -> ProbeReport {
        let mut perm = ln_platform::microphone_permission();
        if perm == Permission::NotDetermined {
            let (tx, rx) = crossbeam_channel::bounded(1);
            ln_platform::request_microphone_permission(move |ok| {
                let _ = tx.send(ok);
            });
            let _ = rx.recv_timeout(Duration::from_secs(60));
            perm = ln_platform::microphone_permission();
        }
        let silero = self.models().file("silero-vad-v5", 0).filter(|p| p.exists());
        let vad_name = if silero.is_some() { "silero" } else { "energy" };
        let mut channels = Vec::new();
        let mut captured: Vec<(Channel, Vec<f32>, u32)> = Vec::new();
        let blank = |channel: Channel| ChannelProbe {
            channel: channel.id().into(),
            started: false,
            error: None,
            sample_rate: 0,
            seconds: 0.0,
            rms: 0.0,
            peak: 0.0,
            nonzero: 0.0,
            sections: 0,
            transcript: vec![],
        };
        // Both channels at once, as in a real recording; if that fails, find out which one fails.
        let both = CaptureConfig { me: Source::Microphone, others: Source::SystemAudio };
        let cap = match Capture::start(&both) {
            Ok(c) => Some(c),
            Err(e) => {
                for (channel, cfg) in [
                    (Channel::Me, CaptureConfig { me: Source::Microphone, others: Source::Off }),
                    (Channel::Others, CaptureConfig { me: Source::Off, others: Source::SystemAudio }),
                ] {
                    let mut p = blank(channel);
                    match Capture::start(&cfg) {
                        Ok(mut c) => {
                            p.started = true;
                            c.stop();
                        }
                        Err(e2) => p.error = Some(e2.to_string()),
                    }
                    channels.push(p);
                }
                tracing::warn!("probe: capture failed: {e}");
                None
            }
        };
        if let Some(mut cap) = cap {
            let mut data: [(Vec<f32>, u32); 2] = [(Vec::new(), 0), (Vec::new(), 0)];
            let end = Instant::now() + Duration::from_secs(seconds);
            while Instant::now() < end {
                if let Ok(t) = cap.audio.recv_timeout(Duration::from_millis(100)) {
                    let i = if t.channel == Channel::Me { 0 } else { 1 };
                    data[i].1 = t.chunk.sample_rate;
                    data[i].0.extend_from_slice(&t.chunk.samples);
                }
            }
            cap.stop();
            for (i, channel) in [Channel::Me, Channel::Others].into_iter().enumerate() {
                let (samples, rate) = std::mem::take(&mut data[i]);
                let mut p = blank(channel);
                p.started = true;
                p.sample_rate = rate;
                p.seconds = if rate > 0 { samples.len() as f32 / rate as f32 } else { 0.0 };
                if !samples.is_empty() {
                    p.rms = (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt();
                    p.peak = samples.iter().fold(0.0f32, |m, x| m.max(x.abs()));
                    p.nonzero = samples.iter().filter(|x| **x != 0.0).count() as f32 / samples.len() as f32;
                }
                captured.push((channel, samples, rate));
                channels.push(p);
            }
        }
        // Segment and transcribe what arrived.
        let mut transcriber_name = None;
        let mut engine = if transcribe {
            self.local_transcriber_for_probe().map(|t| {
                transcriber_name = Some(t.name());
                let s = self.settings();
                AsrEngine::new(t, PostProcessor::new(s.languages.chinese_script), s.languages.mine.clone())
            })
        } else {
            None
        };
        for (channel, samples, rate) in &captured {
            if *rate == 0 {
                continue;
            }
            let vad: Box<dyn VoiceDetector> = {
                #[cfg(feature = "whisper")]
                {
                    match silero.as_ref().and_then(|p| ln_asr::SileroVad::new(p).ok()) {
                        Some(v) => Box::new(v),
                        None => Box::new(ln_asr::EnergyVad::default()),
                    }
                }
                #[cfg(not(feature = "whisper"))]
                {
                    Box::new(ln_asr::EnergyVad::default())
                }
            };
            let mut seg = ChannelSegmenter::new(*channel, vad);
            let mut pending = Vec::new();
            for c in samples.chunks((*rate as usize / 10).max(1)) {
                pending.extend(seg.push(c, *rate));
            }
            pending.extend(seg.flush());
            let p = channels.iter_mut().find(|p| p.channel == channel.id()).unwrap();
            p.sections = pending.len();
            if let Some(e) = engine.as_mut() {
                for s in &pending {
                    if let Ok(Some(x)) = e.process(s, None) {
                        p.transcript.push((x.lang.to_string(), x.text));
                    }
                }
            }
        }
        let findings = findings(perm, &channels, transcribe && engine.is_some());
        ProbeReport {
            microphone_permission: perm_name(perm).into(),
            vad: vad_name.into(),
            transcriber: transcriber_name,
            channels,
            findings,
        }
    }

    fn local_transcriber_for_probe(&self) -> Option<Box<dyn ln_asr::Transcriber>> {
        self.local_transcriber_pub().ok()
    }
}

fn findings(perm: Permission, ch: &[ChannelProbe], transcribed: bool) -> Vec<String> {
    let mut f = Vec::new();
    if perm == Permission::Denied {
        f.push("microphone permission denied: System Settings → Privacy & Security → Microphone → enable Live Notes".into());
    }
    for c in ch {
        let name = if c.channel == "me" { "microphone" } else { "system audio" };
        if let Some(e) = &c.error {
            f.push(format!("{name}: capture failed: {e}"));
        } else if c.sample_rate == 0 {
            f.push(format!("{name}: no audio buffers arrived"));
        } else if c.nonzero == 0.0 {
            if c.channel == "me" {
                f.push("microphone: only digital silence – permission missing or muted input device".into());
            } else {
                f.push("system audio: only digital silence – nothing playing, or permission 'Screen & System Audio Recording → System Audio Only' missing".into());
            }
        } else if c.sections == 0 {
            f.push(format!("{name}: audio arrives (rms {:.4}) but no speech section was detected", c.rms));
        } else if transcribed && c.transcript.is_empty() {
            f.push(format!("{name}: {} speech sections, but transcription returned nothing", c.sections));
        } else {
            f.push(format!("{name}: ok ({} sections)", c.sections));
        }
    }
    f
}
