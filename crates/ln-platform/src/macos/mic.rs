//! Microphone through AVAudioEngine with voice processing (echo cancellation), SPEC §5 F1.

use crate::{AudioChunk, CaptureStream, Error, Result};
use crossbeam_channel::Sender;
use objc2::rc::Retained;
use objc2_avf_audio::{AVAudioEngine, AVAudioPCMBuffer, AVAudioTime};
use std::ptr::NonNull;

struct MicStream {
    engine: Option<Retained<AVAudioEngine>>,
}

// The engine is only touched from `stop`, which runs on the owning thread.
unsafe impl Send for MicStream {}

impl CaptureStream for MicStream {
    fn stop(&mut self) {
        if let Some(e) = self.engine.take() {
            unsafe {
                e.inputNode().removeTapOnBus(0);
                e.stop();
            }
        }
    }
}

impl Drop for MicStream {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn start(tx: Sender<AudioChunk>) -> Result<Box<dyn CaptureStream>> {
    unsafe {
        let engine = AVAudioEngine::new();
        let input = engine.inputNode();
        if let Err(e) = input.setVoiceProcessingEnabled_error(true) {
            // Fall back to a plain mic (as Foresight does) rather than failing the recording.
            tracing::warn!("voice processing unavailable: {}", super::ns_err(&e));
        }
        let format = input.outputFormatForBus(0);
        let rate = format.sampleRate() as u32;
        if rate == 0 {
            return Err(Error::Permission("microphone"));
        }
        let block = block2::RcBlock::new(move |buf: NonNull<AVAudioPCMBuffer>, _when: NonNull<AVAudioTime>| {
            let buf = buf.as_ref();
            let frames = buf.frameLength() as usize;
            let data = buf.floatChannelData();
            if data.is_null() || frames == 0 {
                return;
            }
            // Channel 0 carries the processed voice signal.
            let ch0 = (*data).as_ptr();
            let samples = std::slice::from_raw_parts(ch0, frames).to_vec();
            let _ = tx.try_send(AudioChunk { samples, sample_rate: rate });
        });
        input.installTapOnBus_bufferSize_format_block(0, 4096, Some(&format), &*block as *const _ as *mut _);
        engine.prepare();
        engine.startAndReturnError().map_err(|e| Error::Other(super::ns_err(&e)))?;
        Ok(Box::new(MicStream { engine: Some(engine) }))
    }
}
