mod audio;

pub use audio::start_capture;

use crate::{Error, Result};
use std::path::Path;
use std::process::{Command, Stdio};

pub fn pdf_pages(path: &Path) -> Result<Vec<String>> {
    let path = path.canonicalize().map_err(|error| Error::Other(error.to_string()))?;
    let text = output(Command::new("pdftotext").args(["-enc", "UTF-8"]).arg(&path).arg("-"))?;
    let mut pages: Vec<_> = text.split_terminator('\x0c').map(str::to_owned).collect();
    if pages.iter().any(|page| page.trim().is_empty()) {
        let dir = tempfile::tempdir().map_err(|error| Error::Other(error.to_string()))?;
        let image = dir.path().join("page");
        let languages = ["en-US", "de-DE", "zh-Hans", "zh-Hant", "fr-FR", "es-ES", "it-IT", "pt-BR", "ja-JP", "ko-KR"];
        for (index, text) in pages.iter_mut().enumerate().filter(|(_, text)| text.trim().is_empty()) {
            let page = (index + 1).to_string();
            let render = output(
                Command::new("pdftoppm").args(["-f", &page, "-l", &page, "-r", "200", "-singlefile", "-png"]).arg(&path).arg(&image),
            );
            if let Err(error) = render {
                tracing::warn!("PDF rasterization on page {page}: {error}");
            } else {
                match ocr_image(&image.with_extension("png"), &languages) {
                    Ok(recognized) => *text = recognized,
                    Err(error) => tracing::warn!("PDF OCR on page {page}: {error}"),
                }
            }
        }
    }
    Ok(pages)
}

pub fn ocr_image(path: &Path, languages: &[&str]) -> Result<String> {
    let path = path.canonicalize().map_err(|error| Error::Other(error.to_string()))?;
    let installed = output(Command::new("tesseract").arg("--list-langs"))?;
    let languages: Vec<_> = languages
        .iter()
        .filter_map(|tag| match *tag {
            "zh-Hans" => Some("chi_sim"),
            "zh-Hant" => Some("chi_tra"),
            "de-DE" => Some("deu"),
            "en-US" => Some("eng"),
            "fr-FR" => Some("fra"),
            "es-ES" => Some("spa"),
            "it-IT" => Some("ita"),
            "pt-BR" => Some("por"),
            "ja-JP" => Some("jpn"),
            "ko-KR" => Some("kor"),
            _ => None,
        })
        .filter(|lang| installed.lines().any(|line| line.trim() == *lang))
        .collect();
    let languages = if languages.is_empty() { "eng".into() } else { languages.join("+") };
    output(Command::new("tesseract").arg(path).args(["stdout", "-l", &languages]))
}

pub(super) fn output(command: &mut Command) -> Result<String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command.stdin(Stdio::null()).output().map_err(|error| Error::Other(format!("{program}: {error}")))?;
    if !output.status.success() {
        return Err(Error::Other(format!("{program}: {}", String::from_utf8_lossy(&output.stderr).trim())));
    }
    String::from_utf8(output.stdout).map_err(|error| Error::Other(format!("{program}: {error}")))
}
