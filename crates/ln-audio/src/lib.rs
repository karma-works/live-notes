//! Starts and stops the two recording channels and delivers their audio as one stream of
//! `(Channel, chunk)`. Audio is never written to disk (SPEC §5 F1). For tests and benchmarks
//! a WAV file can stand in for each device.

use crossbeam_channel::{Receiver, Sender, bounded};
use ln_platform::{AudioChunk, CaptureStream};
use ln_types::Channel;
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("microphone: {0}")]
    Mic(ln_platform::Error),
    #[error("system audio: {0}")]
    System(ln_platform::Error),
    #[error("file: {0}")]
    File(String),
}

/// Where a channel's audio comes from.
#[derive(Debug, Clone)]
pub enum Source {
    Microphone,
    SystemAudio,
    /// WAV file, played in real time (`realtime = true`) or as fast as possible.
    File {
        path: PathBuf,
        realtime: bool,
    },
    Off,
}

#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub me: Source,
    pub others: Source,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        CaptureConfig { me: Source::Microphone, others: Source::SystemAudio }
    }
}

/// A tagged chunk.
#[derive(Debug, Clone)]
pub struct Tagged {
    pub channel: Channel,
    pub chunk: AudioChunk,
}

/// Running capture of both channels.
pub struct Capture {
    streams: Vec<Box<dyn CaptureStream>>,
    threads: Vec<JoinHandle<()>>,
    stop_flag: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    muted: Arc<[AtomicBool; 2]>,
    levels: Arc<[AtomicU32; 2]>,
    pub audio: Receiver<Tagged>,
    /// Set when a file source reached its end.
    pub finished: Arc<Mutex<u8>>,
}

fn idx(c: Channel) -> usize {
    match c {
        Channel::Me => 0,
        Channel::Others => 1,
    }
}

impl Capture {
    pub fn start(cfg: &CaptureConfig) -> Result<Capture, Error> {
        let (out_tx, out_rx) = bounded::<Tagged>(512);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let muted: Arc<[AtomicBool; 2]> = Arc::new([AtomicBool::new(false), AtomicBool::new(false)]);
        let levels: Arc<[AtomicU32; 2]> = Arc::new([AtomicU32::new(0), AtomicU32::new(0)]);
        let finished = Arc::new(Mutex::new(0u8));
        let mut cap = Capture { streams: vec![], threads: vec![], stop_flag, paused, muted, levels, audio: out_rx, finished };
        for (channel, src) in [(Channel::Me, &cfg.me), (Channel::Others, &cfg.others)] {
            match src {
                Source::Off => {
                    *cap.finished.lock() += 1;
                }
                Source::Microphone | Source::SystemAudio => {
                    let (tx, rx) = bounded::<AudioChunk>(256);
                    let stream = if matches!(src, Source::Microphone) {
                        ln_platform::start_microphone(tx).map_err(Error::Mic)?
                    } else {
                        ln_platform::start_system_audio(tx).map_err(Error::System)?
                    };
                    cap.streams.push(stream);
                    cap.threads.push(cap.forwarder(channel, rx, out_tx.clone()));
                }
                Source::File { path, realtime } => {
                    let (samples, rate) = read_wav(path)?;
                    let (tx, rx) = bounded::<AudioChunk>(256);
                    let stop = cap.stop_flag.clone();
                    let finished = cap.finished.clone();
                    let realtime = *realtime;
                    cap.threads.push(std::thread::spawn(move || {
                        let block = (rate / 10) as usize;
                        for c in samples.chunks(block) {
                            if stop.load(Ordering::Relaxed) {
                                break;
                            }
                            if tx.send(AudioChunk { samples: c.to_vec(), sample_rate: rate }).is_err() {
                                break;
                            }
                            if realtime {
                                std::thread::sleep(std::time::Duration::from_millis(100));
                            }
                        }
                        *finished.lock() += 1;
                    }));
                    cap.threads.push(cap.forwarder(channel, rx, out_tx.clone()));
                }
            }
        }
        Ok(cap)
    }

    fn forwarder(&self, channel: Channel, rx: Receiver<AudioChunk>, out: Sender<Tagged>) -> JoinHandle<()> {
        let paused = self.paused.clone();
        let muted = self.muted.clone();
        let levels = self.levels.clone();
        let stop = self.stop_flag.clone();
        std::thread::spawn(move || {
            while let Ok(mut chunk) = rx.recv() {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let rms = (chunk.samples.iter().map(|x| x * x).sum::<f32>() / chunk.samples.len().max(1) as f32).sqrt();
                levels[idx(channel)].store(rms.to_bits(), Ordering::Relaxed);
                if paused.load(Ordering::Relaxed) {
                    continue;
                }
                if muted[idx(channel)].load(Ordering::Relaxed) {
                    chunk.samples.iter_mut().for_each(|s| *s = 0.0);
                }
                if out.send(Tagged { channel, chunk }).is_err() {
                    break;
                }
            }
        })
    }

    /// Current RMS level of a channel, 0..1 (for the level meter).
    pub fn level(&self, c: Channel) -> f32 {
        f32::from_bits(self.levels[idx(c)].load(Ordering::Relaxed))
    }

    pub fn set_paused(&self, p: bool) {
        self.paused.store(p, Ordering::Relaxed);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub fn set_muted(&self, c: Channel, m: bool) {
        self.muted[idx(c)].store(m, Ordering::Relaxed);
    }

    /// True once all file sources have been played (always false for live devices).
    pub fn files_done(&self) -> bool {
        *self.finished.lock() >= 2
    }

    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        for s in &mut self.streams {
            s.stop();
        }
        self.streams.clear();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn read_wav(path: &std::path::Path) -> Result<(Vec<f32>, u32), Error> {
    let mut r = hound::WavReader::open(path).map_err(|e| Error::File(e.to_string()))?;
    let spec = r.spec();
    let ch = spec.channels.max(1) as usize;
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().filter_map(Result::ok).collect(),
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>().filter_map(Result::ok).map(|v| v as f32 / max).collect()
        }
    };
    Ok((raw.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect(), spec.sample_rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(path: &std::path::Path, secs: f32) {
        let spec = hound::WavSpec { channels: 1, sample_rate: 16_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..(16_000.0 * secs) as usize {
            w.write_sample(((i as f32 * 0.05).sin() * 8000.0) as i16).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn file_sources_deliver_tagged_audio() {
        let d = std::env::temp_dir().join(format!("ln-audio-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let a = d.join("me.wav");
        wav(&a, 1.0);
        let cfg = CaptureConfig { me: Source::File { path: a.clone(), realtime: false }, others: Source::Off };
        let mut cap = Capture::start(&cfg).unwrap();
        let mut total = 0;
        while let Ok(t) = cap.audio.recv_timeout(std::time::Duration::from_secs(2)) {
            assert_eq!(t.channel, Channel::Me);
            total += t.chunk.samples.len();
            if total >= 16_000 {
                break;
            }
        }
        assert_eq!(total, 16_000);
        assert!(cap.level(Channel::Me) > 0.1);
        cap.stop();
        assert!(cap.files_done());
    }
}
