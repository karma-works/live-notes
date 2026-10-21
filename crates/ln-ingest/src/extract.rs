//! Text extraction per format (SPEC §5 F4 table).

use crate::{Error, Result};
use ln_index::Locator;
use std::io::Read;
use std::path::Path;

/// Bump when extraction changes, so files are re-indexed.
pub const PARSER_VERSION: i64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Markdown,
    Text,
    Html,
    Pdf,
    Docx,
    Pptx,
    Odt,
    Sheet,
    Rtf,
    IWork,
    Image,
    Email,
    /// Audio and video are recognised but not transcribed in v1.0 (SPEC §16).
    Media,
}

impl FileKind {
    pub fn mime(self) -> &'static str {
        match self {
            FileKind::Markdown => "text/markdown",
            FileKind::Text => "text/plain",
            FileKind::Html => "text/html",
            FileKind::Pdf => "application/pdf",
            FileKind::Docx => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            FileKind::Pptx => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            FileKind::Odt => "application/vnd.oasis.opendocument.text",
            FileKind::Sheet => "application/vnd.ms-excel",
            FileKind::Rtf => "text/rtf",
            FileKind::IWork => "application/x-iwork",
            FileKind::Image => "image/*",
            FileKind::Email => "message/rfc822",
            FileKind::Media => "audio/*",
        }
    }
}

pub fn kind_of(path: &Path) -> Option<FileKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "md" | "markdown" | "mdx" => FileKind::Markdown,
        "txt" | "text" | "csv" | "tsv" | "json" | "yaml" | "yml" | "toml" | "log" | "org" | "rst" | "tex" | "rs" | "py" | "js" | "ts"
        | "tsx" | "jsx" | "go" | "java" | "kt" | "swift" | "c" | "h" | "cpp" | "hpp" | "cs" | "rb" | "php" | "sh" | "sql" | "css"
        | "scss" | "xml" | "ini" | "cfg" => FileKind::Text,
        "html" | "htm" => FileKind::Html,
        "pdf" => FileKind::Pdf,
        "docx" => FileKind::Docx,
        "pptx" => FileKind::Pptx,
        "odt" | "odp" => FileKind::Odt,
        "xlsx" | "xlsm" | "xls" | "ods" => FileKind::Sheet,
        "rtf" => FileKind::Rtf,
        "pages" | "key" | "numbers" => FileKind::IWork,
        "png" | "jpg" | "jpeg" | "heic" | "webp" | "tiff" | "gif" => FileKind::Image,
        "eml" => FileKind::Email,
        "m4a" | "mp3" | "wav" | "mp4" | "mov" | "aac" | "flac" | "ogg" => FileKind::Media,
        _ => return None,
    })
}

/// A piece of text with its position in the file.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub text: String,
    pub locator: Locator,
    /// Language known from the source (transcripts carry it per line).
    pub lang: Option<String>,
}

impl Block {
    fn plain(text: impl Into<String>) -> Block {
        Block { text: text.into(), locator: Locator::default(), lang: None }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Extracted {
    pub title: String,
    pub kind: FileKind,
    pub blocks: Vec<Block>,
}

/// Describes an image (Gemma vision), used from v1.1 on (SPEC §16).
pub type ImageDescriber<'a> = &'a (dyn Fn(&Path) -> Option<String> + Sync);

/// Hooks that need other subsystems (OCR languages, image description by Gemma).
#[derive(Default)]
pub struct ExtractOptions<'a> {
    pub ocr_languages: Vec<&'static str>,
    pub describe_image: Option<ImageDescriber<'a>>,
}

pub fn extract(path: &Path, opts: &ExtractOptions<'_>) -> Result<Extracted> {
    let kind = kind_of(path).ok_or(Error::Unsupported)?;
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
    let mut title = stem.clone();
    let blocks = match kind {
        FileKind::Markdown => {
            let text = read_text(path)?;
            let (t, b) = markdown_blocks(&text);
            if let Some(t) = t {
                title = t;
            }
            b
        }
        FileKind::Text => {
            let text = read_text(path)?;
            text_blocks(&text)
        }
        FileKind::Html => {
            let bytes = std::fs::read(path)?;
            if let Some(t) = html_title(&String::from_utf8_lossy(&bytes)) {
                title = t;
            }
            let text = html2text::from_read(&bytes[..], 100).map_err(|e| Error::Parse(e.to_string()))?;
            text_blocks(&text)
        }
        FileKind::Pdf => pdf_blocks(path)?,
        FileKind::Docx => zip_xml(path, &["word/document.xml"], &["w:p"], &["w:t"])?.into_iter().map(|(_, t)| Block::plain(t)).collect(),
        FileKind::Odt => zip_xml(path, &["content.xml"], &["text:p", "text:h"], &["text:p", "text:h", "text:span"])?
            .into_iter()
            .map(|(_, t)| Block::plain(t))
            .collect(),
        FileKind::Pptx => pptx_blocks(path)?,
        FileKind::Sheet => sheet_blocks(path)?,
        FileKind::Rtf => text_blocks(&strip_rtf(&read_text(path)?)),
        FileKind::IWork => iwork_blocks(path)?,
        FileKind::Image => {
            let mut text = ln_platform::ocr_image(path, &opts.ocr_languages).unwrap_or_default();
            if let Some(d) = opts.describe_image.and_then(|f| f(path)) {
                text = format!("{d}\n\n{text}");
            }
            if text.trim().is_empty() { vec![] } else { vec![Block::plain(text)] }
        }
        FileKind::Email => {
            let (t, b) = email_blocks(&std::fs::read(path)?);
            if let Some(t) = t {
                title = t;
            }
            b
        }
        FileKind::Media => return Err(Error::Unsupported),
    };
    let blocks = blocks.into_iter().filter(|b| !b.text.trim().is_empty()).collect();
    Ok(Extracted { title, kind, blocks })
}

fn read_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    if let Ok(s) = std::str::from_utf8(&bytes) {
        return Ok(s.trim_start_matches('\u{feff}').to_string());
    }
    let mut det = chardetng::EncodingDetector::new();
    det.feed(&bytes, true);
    let enc = det.guess(None, true);
    Ok(enc.decode(&bytes).0.into_owned())
}

/// Paragraph blocks with line numbers.
fn text_blocks(text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut start_line = 1u32;
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            if !cur.trim().is_empty() {
                out.push(Block {
                    text: std::mem::take(&mut cur),
                    locator: Locator { line: Some(start_line), ..Default::default() },
                    lang: None,
                });
            }
            cur.clear();
            start_line = i as u32 + 2;
            continue;
        }
        if !cur.is_empty() {
            cur.push('\n');
        }
        cur.push_str(line);
    }
    if !cur.trim().is_empty() {
        out.push(Block { text: cur, locator: Locator { line: Some(start_line), ..Default::default() }, lang: None });
    }
    out
}

/// Sections by heading with heading path; Live Notes transcripts become 60-second windows.
fn markdown_blocks(text: &str) -> (Option<String>, Vec<Block>) {
    let note = ln_notes::Note::parse(text).ok();
    let title = note.as_ref().and_then(|n| n.title());
    let mut blocks = Vec::new();
    // Transcript section: windows of ~60 s or speaker changes.
    if let Some(n) = &note {
        let lines = n.transcript_lines();
        let mut cur: Option<(u64, String, Option<String>, String)> = None; // start, label, lang, text
        for l in lines {
            let flush = match &cur {
                Some((start, label, _, _)) => l.at_ms.saturating_sub(*start) > 60_000 || *label != l.label,
                None => false,
            };
            if flush && let Some((start, _, lang, t)) = cur.take() {
                blocks.push(Block { text: t, locator: Locator { t_ms: Some(start), ..Default::default() }, lang });
            }
            match cur.as_mut() {
                Some((_, _, _, t)) => {
                    t.push(' ');
                    t.push_str(&l.text);
                }
                None => cur = Some((l.at_ms, l.label.clone(), Some(l.lang.to_string()), format!("{}: {}", l.label, l.text))),
            }
        }
        if let Some((start, _, lang, t)) = cur {
            blocks.push(Block { text: t, locator: Locator { t_ms: Some(start), ..Default::default() }, lang });
        }
    }
    // Everything except the transcript, split by headings.
    let body: String = match &note {
        Some(n) => {
            let mut s = n.preamble.clone();
            for sec in n.sections.iter().filter(|s| s.kind != ln_notes::SectionKind::Transcript) {
                s.push_str(&sec.heading);
                s.push('\n');
                s.push_str(&sec.body);
            }
            s
        }
        None => text.to_string(),
    };
    let mut path: Vec<(usize, String)> = Vec::new();
    let mut cur = String::new();
    let mut cur_heading: Option<String> = None;
    let mut in_code = false;
    let push = |cur: &mut String, heading: &Option<String>, blocks: &mut Vec<Block>| {
        if !cur.trim().is_empty() {
            blocks.push(Block {
                text: std::mem::take(cur),
                locator: Locator { heading: heading.clone(), ..Default::default() },
                lang: None,
            });
        }
        cur.clear();
    };
    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
        }
        let hashes = line.chars().take_while(|c| *c == '#').count();
        if !in_code && (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
            push(&mut cur, &cur_heading, &mut blocks);
            let h = line[hashes..].trim().to_string();
            path.retain(|(lvl, _)| *lvl < hashes);
            path.push((hashes, h.clone()));
            cur_heading = Some(path.iter().map(|(_, h)| h.as_str()).collect::<Vec<_>>().join(" > "));
            cur.push_str(&h);
            cur.push('\n');
            continue;
        }
        if line.trim().is_empty() && cur.len() > 1500 {
            push(&mut cur, &cur_heading, &mut blocks);
            continue;
        }
        cur.push_str(line);
        cur.push('\n');
    }
    push(&mut cur, &cur_heading, &mut blocks);
    (title, blocks)
}

fn html_title(html: &str) -> Option<String> {
    let lower = html.to_lowercase();
    let s = lower.find("<title>")? + 7;
    let e = lower[s..].find("</title>")? + s;
    let t = html.get(s..e)?.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn pdf_blocks(path: &Path) -> Result<Vec<Block>> {
    let pages = ln_platform::pdf_pages(path).map_err(|e| Error::Parse(e.to_string()))?;
    Ok(pages
        .into_iter()
        .enumerate()
        .filter(|(_, t)| !t.trim().is_empty())
        .map(|(i, t)| Block { text: t, locator: Locator { page: Some(i as u32 + 1), ..Default::default() }, lang: None })
        .collect())
}

/// Reads XML parts of a zip container and returns paragraphs: text of `text_tags`, split at
/// `para_tags`. Returned with the part index.
fn zip_xml(path: &Path, parts: &[&str], para_tags: &[&str], text_tags: &[&str]) -> Result<Vec<(usize, String)>> {
    let f = std::fs::File::open(path)?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| Error::Parse(e.to_string()))?;
    let mut out = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let mut xml = String::new();
        match z.by_name(part) {
            Ok(mut e) => {
                e.read_to_string(&mut xml)?;
            }
            Err(_) => continue,
        }
        for p in xml_paragraphs(&xml, para_tags, text_tags) {
            out.push((i, p));
        }
    }
    Ok(out)
}

fn xml_paragraphs(xml: &str, para_tags: &[&str], text_tags: &[&str]) -> Vec<String> {
    use quick_xml::events::Event;
    let mut r = quick_xml::Reader::from_str(xml);
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth_text = 0usize;
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.name().as_ref().to_string();
                if text_tags.contains(&name.as_str()) {
                    depth_text += 1;
                }
                if name == "w:tab" || name == "text:tab" {
                    cur.push('\t');
                }
            }
            Ok(Event::Empty(e)) => {
                let name = e.name().as_ref().to_string();
                if matches!(name.as_str(), "w:tab" | "text:tab" | "text:s") {
                    cur.push(' ');
                } else if matches!(name.as_str(), "w:br" | "text:line-break" | "a:br") {
                    cur.push('\n');
                }
            }
            Ok(Event::Text(t)) if depth_text > 0 => {
                cur.push_str(&unescape(&t));
            }
            Ok(Event::GeneralRef(r)) if depth_text > 0 => {
                cur.push_str(&unescape(&format!("&{};", &*r)));
            }
            Ok(Event::End(e)) => {
                let name = e.name().as_ref().to_string();
                if text_tags.contains(&name.as_str()) {
                    depth_text = depth_text.saturating_sub(1);
                }
                if para_tags.contains(&name.as_str()) {
                    if !cur.trim().is_empty() {
                        out.push(cur.trim().to_string());
                    }
                    cur.clear();
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';') else {
            out.push_str(rest);
            return out;
        };
        let ent = &rest[1..end];
        let rep = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e if e.starts_with("#x") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match rep {
            Some(c) => out.push(c),
            None => out.push_str(&rest[..=end]),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

fn pptx_blocks(path: &Path) -> Result<Vec<Block>> {
    let f = std::fs::File::open(path)?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| Error::Parse(e.to_string()))?;
    let mut slides: Vec<(u32, String)> = (0..z.len())
        .filter_map(|i| {
            let name = z.by_index(i).ok()?.name().ok()?.to_string();
            let n = name.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?.parse().ok()?;
            Some((n, name))
        })
        .collect();
    slides.sort();
    let mut out = Vec::new();
    for (n, name) in slides {
        let mut xml = String::new();
        z.by_name(&name).map_err(|e| Error::Parse(e.to_string()))?.read_to_string(&mut xml)?;
        let text = xml_paragraphs(&xml, &["a:p"], &["a:t"]).join("\n");
        out.push(Block { text, locator: Locator { page: Some(n), ..Default::default() }, lang: None });
    }
    Ok(out)
}

fn sheet_blocks(path: &Path) -> Result<Vec<Block>> {
    use calamine::Reader;
    let mut wb = calamine::open_workbook_auto(path).map_err(|e| Error::Parse(e.to_string()))?;
    let mut out = Vec::new();
    for (i, name) in wb.sheet_names().to_vec().iter().enumerate() {
        let Ok(range) = wb.worksheet_range(name) else { continue };
        let mut text = format!("{name}\n");
        for row in range.rows().take(5000) {
            let cells: Vec<String> = row.iter().map(|c| c.to_string()).collect();
            if cells.iter().any(|c| !c.is_empty()) {
                text.push_str(&cells.join("\t"));
                text.push('\n');
            }
        }
        out.push(Block {
            text,
            locator: Locator { page: Some(i as u32 + 1), heading: Some(name.clone()), ..Default::default() },
            lang: None,
        });
    }
    Ok(out)
}

/// iWork packages contain a preview PDF.
fn iwork_blocks(path: &Path) -> Result<Vec<Block>> {
    let preview = if path.is_dir() {
        Some(path.join("QuickLook/Preview.pdf"))
    } else {
        let f = std::fs::File::open(path)?;
        let mut z = zip::ZipArchive::new(f).map_err(|e| Error::Parse(e.to_string()))?;
        let mut tmp = std::env::temp_dir().join(format!("ln-iwork-{}.pdf", std::process::id()));
        match z.by_name("QuickLook/Preview.pdf") {
            Ok(mut e) => {
                let mut buf = Vec::new();
                e.read_to_end(&mut buf)?;
                std::fs::write(&tmp, buf)?;
                Some(std::mem::take(&mut tmp))
            }
            Err(_) => None,
        }
    };
    match preview {
        Some(p) if p.exists() => {
            let r = pdf_blocks(&p);
            if !path.is_dir() {
                let _ = std::fs::remove_file(&p);
            }
            r
        }
        _ => Err(Error::Unsupported),
    }
}

fn email_blocks(bytes: &[u8]) -> (Option<String>, Vec<Block>) {
    let Some(m) = mail_parser::MessageParser::default().parse(bytes) else { return (None, vec![]) };
    let subject = m.subject().map(str::to_string);
    let from =
        m.from().and_then(|a| a.first()).map(|a| format!("{} {}", a.name().unwrap_or(""), a.address().unwrap_or(""))).unwrap_or_default();
    let body = m.body_text(0).map(|b| b.into_owned()).unwrap_or_default();
    let mut blocks = vec![Block::plain(format!("{}\n{}", subject.clone().unwrap_or_default(), from.trim()))];
    blocks.extend(text_blocks(&body));
    (subject, blocks)
}

/// Very small RTF to text: drops control words and groups.
fn strip_rtf(rtf: &str) -> String {
    let mut out = String::new();
    let mut chars = rtf.chars().peekable();
    let mut depth_skip: Option<usize> = None;
    let mut depth = 0usize;
    while let Some(c) = chars.next() {
        match c {
            '{' => depth += 1,
            '}' => {
                if depth_skip == Some(depth) {
                    depth_skip = None;
                }
                depth = depth.saturating_sub(1);
            }
            '\\' => {
                let mut word = String::new();
                while let Some(&n) = chars.peek() {
                    if n.is_ascii_alphabetic() {
                        word.push(n);
                        chars.next();
                    } else {
                        break;
                    }
                }
                while let Some(&n) = chars.peek() {
                    if n.is_ascii_digit() || n == '-' {
                        chars.next();
                    } else {
                        break;
                    }
                }
                if chars.peek() == Some(&' ') {
                    chars.next();
                }
                match word.as_str() {
                    "par" | "line" => out.push('\n'),
                    "tab" => out.push('\t'),
                    "fonttbl" | "colortbl" | "stylesheet" | "info" | "pict" => depth_skip = Some(depth),
                    "" => {
                        if let Some(n) = chars.next()
                            && depth_skip.is_none()
                            && n != '\''
                        {
                            out.push(n)
                        }
                    }
                    _ => {}
                }
            }
            '\r' | '\n' => {}
            _ if depth_skip.is_none() => out.push(c),
            _ => {}
        }
    }
    out
}

/// ISO 639-1 language of a text, if reliable.
pub fn detect_lang(text: &str) -> Option<String> {
    let info = whatlang::detect(text)?;
    if !info.is_reliable() && text.chars().count() > 40 {
        return None;
    }
    let code = match info.lang() {
        whatlang::Lang::Deu => "de",
        whatlang::Lang::Eng => "en",
        whatlang::Lang::Cmn => "zh",
        whatlang::Lang::Fra => "fr",
        whatlang::Lang::Spa => "es",
        whatlang::Lang::Ita => "it",
        whatlang::Lang::Por => "pt",
        whatlang::Lang::Nld => "nl",
        whatlang::Lang::Jpn => "ja",
        whatlang::Lang::Kor => "ko",
        whatlang::Lang::Rus => "ru",
        whatlang::Lang::Pol => "pl",
        whatlang::Lang::Tur => "tr",
        whatlang::Lang::Ara => "ar",
        _ => return None,
    };
    Some(code.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn markdown_with_headings_and_transcript() {
        let md = "---\ntitle: Teamrunde\n---\n\n<!-- ln:notes -->\n## Meine Notizen\n- Budget\n\n<!-- ln:transcript -->\n## Transkript\n[00:00:01] Ich (de): Hallo\n[00:00:05] Ich (de): Budget Q1\n[00:00:09] Andere (zh): 大家好\n";
        let (title, blocks) = markdown_blocks(md);
        assert_eq!(title.unwrap(), "Teamrunde");
        let transcript: Vec<_> = blocks.iter().filter(|b| b.locator.t_ms.is_some()).collect();
        assert_eq!(transcript.len(), 2, "speaker change starts a new window");
        assert_eq!(transcript[0].text, "Ich: Hallo Budget Q1");
        assert_eq!(transcript[1].lang.as_deref(), Some("zh"));
        assert!(blocks.iter().any(|b| b.locator.heading.as_deref() == Some("Meine Notizen") && b.text.contains("Budget")));
    }

    #[test]
    fn manual_markdown_heading_path() {
        let (title, blocks) = markdown_blocks("# Projekt\nIntro\n## Budget\nZahlen\n```\n# kein Titel\n```\n");
        assert_eq!(title.unwrap(), "Projekt");
        assert_eq!(blocks.last().unwrap().locator.heading.as_deref(), Some("Projekt > Budget"));
        assert!(blocks.last().unwrap().text.contains("# kein Titel"));
    }

    #[test]
    fn docx_xlsx_like_and_rtf() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.docx");
        {
            let f = std::fs::File::create(&p).unwrap();
            let mut z = zip::ZipWriter::new(f);
            z.start_file("word/document.xml", zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(r#"<w:document><w:body><w:p><w:r><w:t>Angebot &amp; Preis</w:t></w:r></w:p><w:p><w:r><w:t>报价</w:t></w:r></w:p></w:body></w:document>"#.as_bytes()).unwrap();
            z.finish().unwrap();
        }
        let e = extract(&p, &ExtractOptions::default()).unwrap();
        assert_eq!(e.blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), vec!["Angebot & Preis", "报价"]);
        assert_eq!(e.title, "a");
        assert_eq!(strip_rtf(r"{\rtf1{\fonttbl{\f0 Helvetica;}}\f0 Hallo\par Welt}").trim(), "Hallo\nWelt");
    }

    #[test]
    fn email_and_text_and_lang() {
        let (s, b) = email_blocks(b"Subject: Angebot\r\nFrom: Li <li@example.com>\r\n\r\nDer Preis gilt bis Ende Q1.\r\n");
        assert_eq!(s.unwrap(), "Angebot");
        assert!(b.iter().any(|x| x.text.contains("Ende Q1")));
        let t = text_blocks("a\nb\n\n\nc");
        assert_eq!(t.len(), 2);
        assert_eq!(t[1].locator.line, Some(5));
        assert_eq!(detect_lang("Das Budget für das nächste Quartal wird nächste Woche besprochen.").as_deref(), Some("de"));
        assert_eq!(detect_lang("我们下周再讨论预算，我周五前把报价发给你。").as_deref(), Some("zh"));
        assert_eq!(kind_of(Path::new("x.HEIC")), Some(FileKind::Image));
    }
}
