//! System audio via a Core Audio process tap (macOS 14.2+), SPEC §5 F1.
//!
//! Steps: global stereo tap excluding our own process → private aggregate device that
//! contains the default output device and the tap → IO block receives the tapped audio.

use crate::{AudioChunk, CaptureStream, Error, Result};
use crossbeam_channel::Sender;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_core_audio::*;
use objc2_core_audio_types::{AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp};
use objc2_core_foundation::{CFDictionary, CFString};
use objc2_foundation::{NSArray, NSDictionary, NSMutableDictionary, NSNumber, NSObject, NSString, NSUUID};
use std::ffi::c_void;
use std::ptr::NonNull;

struct TapStream {
    tap: AudioObjectID,
    aggregate: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    _block: block2::RcBlock<
        dyn Fn(NonNull<AudioTimeStamp>, NonNull<AudioBufferList>, NonNull<AudioTimeStamp>, NonNull<AudioBufferList>, NonNull<AudioTimeStamp>),
    >,
    running: bool,
}

unsafe impl Send for TapStream {}

impl CaptureStream for TapStream {
    fn stop(&mut self) {
        if !self.running {
            return;
        }
        self.running = false;
        unsafe {
            AudioDeviceStop(self.aggregate, self.proc_id);
            AudioDeviceDestroyIOProcID(self.aggregate, self.proc_id);
            AudioHardwareDestroyAggregateDevice(self.aggregate);
            AudioHardwareDestroyProcessTap(self.tap);
        }
    }
}

impl Drop for TapStream {
    fn drop(&mut self) {
        self.stop();
    }
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

pub fn start(tx: Sender<AudioChunk>) -> Result<Box<dyn CaptureStream>> {
    unsafe {
        // Exclude our own process so app sounds never end up in the transcript.
        let pid = std::process::id() as i32;
        let own: AudioObjectID = get_property(kAudioObjectSystemObject as AudioObjectID, kAudioHardwarePropertyTranslatePIDToProcessObject, Some(&pid.to_ne_bytes())).unwrap_or(0);
        let excluded: Retained<NSArray<NSNumber>> = if own != 0 {
            NSArray::from_retained_slice(&[NSNumber::new_u32(own)])
        } else {
            NSArray::new()
        };
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

        let out_dev: AudioObjectID = get_property(kAudioObjectSystemObject as AudioObjectID, kAudioHardwarePropertyDefaultSystemOutputDevice, None)?;
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
        if let Err(e) = check("AudioHardwareCreateAggregateDevice", AudioHardwareCreateAggregateDevice(cf_dict, NonNull::from(&mut aggregate))) {
            AudioHardwareDestroyProcessTap(tap);
            return Err(e);
        }

        let rate = format.mSampleRate as u32;
        let channels = format.mChannelsPerFrame.max(1) as usize;
        let interleaved = format.mFormatFlags & objc2_core_audio_types::kAudioFormatFlagIsNonInterleaved == 0;
        let block = block2::RcBlock::new(
            move |_now: NonNull<AudioTimeStamp>, input: NonNull<AudioBufferList>, _t: NonNull<AudioTimeStamp>, _o: NonNull<AudioBufferList>, _ot: NonNull<AudioTimeStamp>| {
                let list = input.as_ref();
                let n = list.mNumberBuffers as usize;
                let bufs = std::slice::from_raw_parts(list.mBuffers.as_ptr(), n);
                let mono: Vec<f32> = if interleaved && n >= 1 {
                    let b = &bufs[0];
                    let total = b.mDataByteSize as usize / 4;
                    let data = std::slice::from_raw_parts(b.mData as *const f32, total);
                    data.chunks(channels).map(|f| f.iter().sum::<f32>() / f.len() as f32).collect()
                } else if n >= 1 {
                    let frames = bufs[0].mDataByteSize as usize / 4;
                    let mut acc = vec![0.0f32; frames];
                    for b in bufs {
                        let d = std::slice::from_raw_parts(b.mData as *const f32, (b.mDataByteSize as usize / 4).min(frames));
                        for (a, x) in acc.iter_mut().zip(d) {
                            *a += *x;
                        }
                    }
                    acc.iter_mut().for_each(|a| *a /= n as f32);
                    acc
                } else {
                    return;
                };
                if !mono.is_empty() {
                    let _ = tx.try_send(AudioChunk { samples: mono, sample_rate: rate });
                }
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
        Ok(Box::new(TapStream { tap, aggregate, proc_id, _block: block, running: true }))
    }
}
