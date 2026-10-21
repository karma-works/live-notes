//! Microphone through AVAudioEngine with voice processing (echo cancellation), SPEC §5 F1.
//!
//! Field findings (verified with `live-notes --probe-capture` on a Bluetooth headset):
//! - Opening the microphone of a Bluetooth headset switches it to its call profile. The
//!   configuration change stops the engine, so the first start delivers no buffers; a second
//!   start after the switch works. → Retry, and restart whenever buffers stop arriving.
//! - Voice processing can fail to initialise (-10875) on some devices. → Fall back to the plain
//!   input (no echo cancellation), as Foresight does.
//! - Without voice processing the engine only pulls the input if mixer → output is connected.
//! - Voice processing ducks other audio by default, which also silences the system-audio tap.
//!   → Minimum ducking.

use crate::{AudioChunk, CaptureStream, Error, Result};
use crossbeam_channel::Sender;
use objc2::rc::Retained;
use objc2_avf_audio::{
    AVAudioEngine, AVAudioPCMBuffer, AVAudioTime, AVAudioVoiceProcessingOtherAudioDuckingConfiguration,
    AVAudioVoiceProcessingOtherAudioDuckingLevel,
};
use parking_lot::Mutex;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

struct Engine {
    engine: Retained<AVAudioEngine>,
    alive: Arc<AtomicBool>,
    voice_processing: bool,
}

// Only used under the mutex; AVAudioEngine may be stopped from any thread.
unsafe impl Send for Engine {}

impl Engine {
    fn close(self) {
        self.alive.store(false, Ordering::Relaxed);
        unsafe {
            self.engine.inputNode().removeTapOnBus(0);
            self.engine.stop();
        }
    }
}

struct MicStream {
    engine: Arc<Mutex<Option<Engine>>>,
    stop: Arc<AtomicBool>,
    watcher: Option<std::thread::JoinHandle<()>>,
}

impl CaptureStream for MicStream {
    fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.watcher.take() {
            let _ = w.join();
        }
        if let Some(e) = self.engine.lock().take() {
            e.close();
        }
    }
}

impl Drop for MicStream {
    fn drop(&mut self) {
        self.stop();
    }
}

fn first_buffer_timeout(voice_processing: bool) -> Duration {
    Duration::from_millis(if voice_processing { 4000 } else { 3000 })
}

/// Buffers stopped for this long → the device changed under us; restart.
const STALL: Duration = Duration::from_millis(2500);

pub fn start(tx: Sender<AudioChunk>) -> Result<Box<dyn CaptureStream>> {
    let buffers = Arc::new(AtomicU64::new(0));
    let mut last_err = None;
    let mut opened = None;
    for vp in [true, true, false] {
        match open(tx.clone(), vp, buffers.clone()) {
            Ok(e) => {
                opened = Some(e);
                break;
            }
            Err(e) => {
                tracing::warn!("microphone start (voice processing {vp}) failed: {e}");
                last_err = Some(e);
            }
        }
    }
    let Some(first) = opened else { return Err(last_err.unwrap_or(Error::Other("microphone".into()))) };
    let vp = first.voice_processing;
    let engine = Arc::new(Mutex::new(Some(first)));
    let stop = Arc::new(AtomicBool::new(false));
    let watcher = {
        let engine = engine.clone();
        let stop = stop.clone();
        std::thread::Builder::new()
            .name("ln-mic-watch".into())
            .spawn(move || {
                let mut seen = buffers.load(Ordering::Relaxed);
                let mut last_progress = Instant::now();
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(250));
                    let now = buffers.load(Ordering::Relaxed);
                    if now != seen {
                        seen = now;
                        last_progress = Instant::now();
                        continue;
                    }
                    if last_progress.elapsed() < STALL || stop.load(Ordering::Relaxed) {
                        continue;
                    }
                    tracing::info!("microphone stalled: restarting");
                    let mut g = engine.lock();
                    if let Some(old) = g.take() {
                        old.close();
                    }
                    for v in [vp, false] {
                        match open(tx.clone(), v, buffers.clone()) {
                            Ok(e) => {
                                *g = Some(e);
                                break;
                            }
                            Err(e) => tracing::warn!("microphone restart failed: {e}"),
                        }
                    }
                    seen = buffers.load(Ordering::Relaxed);
                    last_progress = Instant::now();
                }
            })
            .ok()
    };
    Ok(Box::new(MicStream { engine, stop, watcher }))
}

/// Starts an engine and waits for its first buffer.
fn open(tx: Sender<AudioChunk>, voice_processing: bool, buffers: Arc<AtomicU64>) -> Result<Engine> {
    let alive = Arc::new(AtomicBool::new(true));
    let start_count = buffers.load(Ordering::Relaxed);
    let engine = unsafe {
        let engine = AVAudioEngine::new();
        let input = engine.inputNode();
        if voice_processing {
            if let Err(e) = input.setVoiceProcessingEnabled_error(true) {
                return Err(Error::Other(format!("voice processing: {}", super::ns_err(&e))));
            }
            input.setVoiceProcessingOtherAudioDuckingConfiguration(AVAudioVoiceProcessingOtherAudioDuckingConfiguration {
                enableAdvancedDucking: objc2::runtime::Bool::NO,
                duckingLevel: AVAudioVoiceProcessingOtherAudioDuckingLevel::Min,
            });
        } else {
            let mixer = engine.mainMixerNode();
            mixer.setOutputVolume(0.0);
        }
        let format = input.outputFormatForBus(0);
        let rate = format.sampleRate() as u32;
        if rate == 0 {
            return Err(Error::Permission("microphone"));
        }
        let count = buffers.clone();
        let alive2 = alive.clone();
        let block = block2::RcBlock::new(move |buf: NonNull<AVAudioPCMBuffer>, _when: NonNull<AVAudioTime>| {
            if !alive2.load(Ordering::Relaxed) {
                return;
            }
            let buf = buf.as_ref();
            let frames = buf.frameLength() as usize;
            let data = buf.floatChannelData();
            if data.is_null() || frames == 0 {
                return;
            }
            count.fetch_add(1, Ordering::Relaxed);
            // Channel 0 carries the (processed) voice signal.
            let ch0 = (*data).as_ptr();
            let samples = std::slice::from_raw_parts(ch0, frames).to_vec();
            let _ = tx.try_send(AudioChunk { samples, sample_rate: rate });
        });
        input.installTapOnBus_bufferSize_format_block(0, 4096, Some(&format), &*block as *const _ as *mut _);
        engine.prepare();
        if let Err(e) = engine.startAndReturnError() {
            input.removeTapOnBus(0);
            return Err(Error::Other(super::ns_err(&e)));
        }
        tracing::info!("microphone started: {rate} Hz, voice processing {voice_processing}");
        Engine { engine, alive, voice_processing }
    };
    let deadline = Instant::now() + first_buffer_timeout(voice_processing);
    while Instant::now() < deadline {
        if buffers.load(Ordering::Relaxed) > start_count {
            return Ok(engine);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    engine.close();
    Err(Error::Other(format!("no microphone buffers within {} ms", first_buffer_timeout(voice_processing).as_millis())))
}
