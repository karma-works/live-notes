use crate::Permission;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};

pub fn microphone() -> Permission {
    let Some(media) = (unsafe { AVMediaTypeAudio }) else { return Permission::NotDetermined };
    let st = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media) };
    if st == AVAuthorizationStatus::Authorized {
        Permission::Granted
    } else if st == AVAuthorizationStatus::NotDetermined {
        Permission::NotDetermined
    } else {
        Permission::Denied
    }
}

pub fn request_microphone(done: impl Fn(bool) + Send + Sync + 'static) {
    let Some(media) = (unsafe { AVMediaTypeAudio }) else {
        done(false);
        return;
    };
    let block = block2::RcBlock::new(move |granted: objc2::runtime::Bool| done(granted.as_bool()));
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(media, &block) };
}
