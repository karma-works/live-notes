pub mod mic;
pub mod ocr;
pub mod pdf;
pub mod permissions;
pub mod system_audio;

use objc2_foundation::NSString;

pub(crate) fn ns_err(e: &objc2_foundation::NSError) -> String {
    e.localizedDescription().to_string()
}

pub(crate) fn file_url(path: &std::path::Path) -> objc2::rc::Retained<objc2_foundation::NSURL> {
    let s = NSString::from_str(&path.to_string_lossy());
    objc2_foundation::NSURL::fileURLWithPath(&s)
}
