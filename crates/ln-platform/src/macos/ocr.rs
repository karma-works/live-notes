//! Text recognition with the Vision framework (works offline, includes Chinese).

use crate::{Error, Result};
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_foundation::{NSArray, NSDictionary, NSString};
use objc2_vision::{VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedTextObservation, VNRequest, VNRequestTextRecognitionLevel};

pub fn recognize(path: &std::path::Path, languages: &[&str]) -> Result<String> {
    objc2::rc::autoreleasepool(|_| unsafe {
        let url = super::file_url(path);
        let handler = VNImageRequestHandler::initWithURL_options(VNImageRequestHandler::alloc(), &url, &NSDictionary::new());
        let req = VNRecognizeTextRequest::new();
        req.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        req.setUsesLanguageCorrection(true);
        if !languages.is_empty() {
            let langs: Vec<Retained<NSString>> = languages.iter().map(|l| NSString::from_str(l)).collect();
            req.setRecognitionLanguages(&NSArray::from_retained_slice(&langs));
        } else {
            req.setAutomaticallyDetectsLanguage(true);
        }
        let as_req: Retained<VNRequest> = Retained::into_super(Retained::into_super(req.clone()));
        handler.performRequests_error(&NSArray::from_retained_slice(&[as_req])).map_err(|e| Error::Other(super::ns_err(&e)))?;
        let mut lines = Vec::new();
        if let Some(results) = req.results() {
            for obs in results.iter() {
                let obs: &VNRecognizedTextObservation = &obs;
                if let Some(top) = obs.topCandidates(1).firstObject() {
                    lines.push(top.string().to_string());
                }
            }
        }
        Ok(lines.join("\n"))
    })
}
