use crate::{Error, Result};
use objc2::AnyThread;
use objc2_pdf_kit::PDFDocument;

pub fn pages(path: &std::path::Path) -> Result<Vec<String>> {
    objc2::rc::autoreleasepool(|_| unsafe {
        let url = super::file_url(path);
        let doc = PDFDocument::initWithURL(PDFDocument::alloc(), &url).ok_or_else(|| Error::Other(format!("cannot open {}", path.display())))?;
        let n = doc.pageCount();
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let text = doc.pageAtIndex(i).and_then(|p| p.string()).map(|s| s.to_string()).unwrap_or_default();
            out.push(text);
        }
        Ok(out)
    })
}
