//! System audio via a Core Audio process tap (macOS 14.2+), SPEC §5 F1.
//!
//! Steps: global stereo tap excluding our own process → private aggregate device that
//! contains the default output device and the tap → IO block receives the tapped audio.
//!
//! The aggregate is bound to the output device. When that changes – headphones plugged in, or a
//! Bluetooth headset switching to its 16 kHz call profile because a microphone opened – the old
//! aggregate only delivers silence. A watcher rebuilds tap and aggregate when the default output
//! device or its sample rate changes.

use crate::{AudioChunk, CaptureStream, Error, Result};
use crossbeam_channel::Sender;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_core_audio::*;
use objc2_core_audio_types::{AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp};
use objc2_core_foundation::{CFDictionary, CFString};
use objc2_foundation::{NSArray, NSDictionary, NSMutableDictionary, NSNumber, NSObject, NSString, NSUUID};
use parking_lot::Mutex;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The IO callback: (now, input data, input time, output data, output time).
type IoBlock = block2::RcBlock<
    dyn Fn(NonNull<AudioTimeStamp>, NonNull<AudioBufferList>, NonNull<AudioTimeStamp>, NonNull<AudioBufferList>, NonNull<AudioTimeStamp>),
>;

/// One tap + aggregate device + IO proc, bound to one output device configuration.
struct Session {
    tap: AudioObjectID,
    aggregate: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    _block: IoBlock,
    device: (AudioObjectID, u32),
}

// Only touched under the mutex; Core Audio objects are thread-agnostic.
unsafe impl Send for Session {}

impl Session {
    fn close(self) {
        unsafe {
            AudioDeviceStop(self.aggregate, self.proc_id);
            AudioDeviceDestroyIOProcID(self.aggregate, self.proc_id);
            AudioHardwareDestroyAggregateDevice(self.aggregate);
            AudioHardwareDestroyProcessTap(self.tap);
        }
    }
}

struct TapStream {
    session: Arc<Mutex<Option<Session>>>,
    stop: Arc<AtomicBool>,
    watcher: Option<std::thread::JoinHandle<()>>,
}

impl CaptureStream for TapStream {
    fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.watcher.take() {
            let _ = w.join();
        }
        if let Some(s) = self.session.lock().take() {
            s.close();
        }
    }
}

impl Drop for TapStream {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Default output device and its nominal sample rate.
fn output_device() -> Option<(AudioObjectID, u32)> {
    unsafe {
        let dev: AudioObjectID =
            get_property(kAudioObjectSystemObject as AudioObjectID, kAudioHardwarePropertyDefaultOutputDevice, None).ok()?;
        let rate: f64 = get_property(dev, kAudioDevicePropertyNominalSampleRate, None).unwrap_or(0.0);
        Some((dev, rate as u32))
    }
}

pub fn start(tx: Sender<AudioChunk>) -> Result<Box<dyn CaptureStream>> {
    let first = open(tx.clone())?;
    tracing::info!("system audio tap started on output device {:?}", first.device);
    let session = Arc::new(Mutex::new(Some(first)));
    let stop = Arc::new(AtomicBool::new(false));
    let watcher = {
        let session = session.clone();
        let stop = stop.clone();
        std::thread::Builder::new()
            .name("ln-tap-watch".into())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(500));
                    let Some(now) = output_device() else { continue };
                    let current = session.lock().as_ref().map(|s| s.device);
                    if current == Some(now) || stop.load(Ordering::Relaxed) {
                        continue;
                    }
                    tracing::info!("output device changed {current:?} → {now:?}: rebuilding system audio tap");
                    // Let the device finish switching (Bluetooth profile changes take a moment).
                    std::thread::sleep(Duration::from_millis(700));
                    let mut g = session.lock();
                    if let Some(old) = g.take() {
                        old.close();
                    }
                    match open(tx.clone()) {
                        Ok(s) => *g = Some(s),
                        Err(e) => tracing::warn!("rebuilding system audio tap failed: {e}"),
                    }
                }
            })
            .ok()
    };
    Ok(Box::new(TapStream { session, stop, watcher }))
}

fn check(what: &'static str, status: i32) -> Result<()> {
    if status == 0 { Ok(()) } else { Err(Error::Os { what, status }) }
}

unsafe fn get_property<T: Copy>(obj: AudioObjectID, selector: u32, qualifier: Option<&[u8]>) -> Result<T> {
    let mut addr = AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut out = std::mem::MaybeUninit::<T>::zeroed();
    let mut size = std::mem::size_of::<T>() as u32;
    let (qs, qp) = match qualifier {
        Some(q) => (q.len() as u32, q.as_ptr() as *const c_void),
        None => (0, std::ptr::null()),
    };
    let st = unsafe {
        AudioObjectGetPropertyData(
            obj,
            NonNull::from(&mut addr),
            qs,
            qp,
            NonNull::from(&mut size),
            NonNull::new_unchecked(out.as_mut_ptr() as *mut c_void),
        )
    };
    check("AudioObjectGetPropertyData", st)?;
    Ok(unsafe { out.assume_init() })
}

unsafe fn get_string(obj: AudioObjectID, selector: u32) -> Result<String> {
    let cf: *const CFString = unsafe { get_property(obj, selector, None)? };
    if cf.is_null() {
        return Err(Error::Other("empty CFString".into()));
    }
    let s = unsafe { (*cf).to_string() };
    unsafe { objc2_core_foundation::CFRetained::from_raw(NonNull::new_unchecked(cf as *mut CFString)) };
    Ok(s)
}

fn open(tx: Sender<AudioChunk>) -> Result<Session> {
    unsafe {
        let device = output_device().unwrap_or((0, 0));
        // Exclude our own process so app sounds never end up in the transcript.
        let pid = std::process::id() as i32;
        let own: AudioObjectID = get_property(
            kAudioObjectSystemObject as AudioObjectID,
            kAudioHardwarePropertyTranslatePIDToProcessObject,
            Some(&pid.to_ne_bytes()),
        )
        .unwrap_or(0);
        let excluded: Retained<NSArray<NSNumber>> =
            if own != 0 { NSArray::from_retained_slice(&[NSNumber::new_u32(own)]) } else { NSArray::new() };
        let desc = CATapDescription::initStereoGlobalTapButExcludeProcesses(CATapDescription::alloc(), &excluded);
        let uuid = NSUUID::new();
        desc.setUUID(&uuid);
        desc.setName(&NSString::from_str("Live Notes"));
        desc.setPrivate(true);
        desc.setMuteBehavior(CATapMuteBehavior::Unmuted);

        let mut tap: AudioObjectID = 0;
        check("AudioHardwareCreateProcessTap", AudioHardwareCreateProcessTap(Some(&desc), &mut tap))
            .map_err(|_| Error::Permission("system audio"))?;

        let format: AudioStreamBasicDescription = match get_property(tap, kAudioTapPropertyFormat, None) {
            Ok(f) => f,
            Err(e) => {
                AudioHardwareDestroyProcessTap(tap);
                return Err(e);
            }
        };

        let out_dev: AudioObjectID =
            get_property(kAudioObjectSystemObject as AudioObjectID, kAudioHardwarePropertyDefaultOutputDevice, None)?;
        let out_uid = get_string(out_dev, kAudioDevicePropertyDeviceUID)?;
        let tap_uid = uuid.UUIDString().to_string();
        let agg_uid = NSUUID::new().UUIDString().to_string();

        let key = |c: &std::ffi::CStr| NSString::from_str(c.to_str().unwrap());
        let sub = NSDictionary::<NSString, NSObject>::from_retained_objects(
            &[&*key(kAudioSubDeviceUIDKey)],
            &[Retained::into_super(NSString::from_str(&out_uid))],
        );
        let tap_entry = NSDictionary::<NSString, NSObject>::from_retained_objects(
            &[&*key(kAudioSubTapDriftCompensationKey), &*key(kAudioSubTapUIDKey)],
            &[Retained::into_super(Retained::into_super(NSNumber::new_bool(true))), Retained::into_super(NSString::from_str(&tap_uid))],
        );
        let dict = NSMutableDictionary::<NSString, NSObject>::new();
        let put = |k: &std::ffi::CStr, v: Retained<NSObject>| dict.insert(&*key(k), &*v);
        put(kAudioAggregateDeviceNameKey, Retained::into_super(NSString::from_str("Live Notes Tap")));
        put(kAudioAggregateDeviceUIDKey, Retained::into_super(NSString::from_str(&agg_uid)));
        put(kAudioAggregateDeviceMainSubDeviceKey, Retained::into_super(NSString::from_str(&out_uid)));
        put(kAudioAggregateDeviceIsPrivateKey, Retained::into_super(Retained::into_super(NSNumber::new_bool(true))));
        put(kAudioAggregateDeviceIsStackedKey, Retained::into_super(Retained::into_super(NSNumber::new_bool(false))));
        put(kAudioAggregateDeviceTapAutoStartKey, Retained::into_super(Retained::into_super(NSNumber::new_bool(true))));
        put(kAudioAggregateDeviceSubDeviceListKey, Retained::into_super(NSArray::from_retained_slice(&[sub])));
        put(kAudioAggregateDeviceTapListKey, Retained::into_super(NSArray::from_retained_slice(&[tap_entry])));

        let cf_dict: &CFDictionary = &*(Retained::as_ptr(&dict) as *const CFDictionary);
        let mut aggregate: AudioObjectID = 0;
        if let Err(e) =
            check("AudioHardwareCreateAggregateDevice", AudioHardwareCreateAggregateDevice(cf_dict, NonNull::from(&mut aggregate)))
        {
            AudioHardwareDestroyProcessTap(tap);
            return Err(e);
        }

        // The real rate is the aggregate's (a Bluetooth headset in call mode runs at 16 kHz while
        // the tap still reports 48 kHz). Channel layout is read from each buffer, not the format.
        let agg_rate: f64 = get_property(aggregate, kAudioDevicePropertyNominalSampleRate, None).unwrap_or(0.0);
        let rate = if agg_rate > 0.0 { agg_rate as u32 } else { format.mSampleRate as u32 };
        tracing::info!("system audio: tap format {} Hz × {} ch, aggregate {} Hz", format.mSampleRate, format.mChannelsPerFrame, agg_rate);
        let block = block2::RcBlock::new(
            move |_now: NonNull<AudioTimeStamp>,
                  input: NonNull<AudioBufferList>,
                  _t: NonNull<AudioTimeStamp>,
                  _o: NonNull<AudioBufferList>,
                  _ot: NonNull<AudioTimeStamp>| {
                let list = input.as_ref();
                let n = list.mNumberBuffers as usize;
                if n == 0 {
                    return;
                }
                let bufs = std::slice::from_raw_parts(list.mBuffers.as_ptr(), n);
                // Frames per buffer = bytes / 4 / channels-in-that-buffer; all buffers carry the
                // same number of frames. Mix every channel of every buffer down to mono.
                let frames = bufs.iter().map(|b| b.mDataByteSize as usize / 4 / (b.mNumberChannels.max(1) as usize)).min().unwrap_or(0);
                if frames == 0 {
                    return;
                }
                let mut acc = vec![0.0f32; frames];
                let mut total_ch = 0usize;
                for b in bufs {
                    let ch = b.mNumberChannels.max(1) as usize;
                    if b.mData.is_null() {
                        continue;
                    }
                    let d = std::slice::from_raw_parts(b.mData as *const f32, frames * ch);
                    for (i, a) in acc.iter_mut().enumerate() {
                        for c in 0..ch {
                            *a += d[i * ch + c];
                        }
                    }
                    total_ch += ch;
                }
                if total_ch == 0 {
                    return;
                }
                acc.iter_mut().for_each(|a| *a /= total_ch as f32);
                let _ = tx.try_send(AudioChunk { samples: acc, sample_rate: rate });
            },
        );
        let mut proc_id: AudioDeviceIOProcID = None;
        let st = AudioDeviceCreateIOProcIDWithBlock(NonNull::from(&mut proc_id), aggregate, None, &*block as *const _ as *mut _);
        if let Err(e) = check("AudioDeviceCreateIOProcIDWithBlock", st) {
            AudioHardwareDestroyAggregateDevice(aggregate);
            AudioHardwareDestroyProcessTap(tap);
            return Err(e);
        }
        if let Err(e) = check("AudioDeviceStart", AudioDeviceStart(aggregate, proc_id)) {
            AudioDeviceDestroyIOProcID(aggregate, proc_id);
            AudioHardwareDestroyAggregateDevice(aggregate);
            AudioHardwareDestroyProcessTap(tap);
            return Err(e);
        }
        Ok(Session { tap, aggregate, proc_id, _block: block, device })
    }
}
