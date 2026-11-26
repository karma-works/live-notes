//! Images in notes: content-addressed copies in `assets/` next to the note and the Markdown
//! links that point at them (SPEC §5 F10).

use crate::Result;
use std::path::{Path, PathBuf};

/// Folder next to a note that holds its images.
pub const ASSETS_DIR: &str = "assets";

/// File extensions accepted as images (lower case).
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp"];

/// Whether `path` names an image by its extension.
pub fn is_image_path(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Stores `bytes` as `<dir>/assets/<first 12 hex of blake3>.<ext>` and returns the path relative
/// to `dir` (`assets/…`). The same image stored twice is written once.
pub fn store_asset(dir: &Path, bytes: &[u8], ext: &str) -> Result<String> {
    let hash = blake3::hash(bytes).to_hex();
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    let name = format!("{}.{ext}", &hash.as_str()[..12]);
    let assets = dir.join(ASSETS_DIR);
    std::fs::create_dir_all(&assets)?;
    let dest = assets.join(&name);
    if !dest.exists() {
        // Write next to the target and rename, so a half-written image never appears.
        let mut tmp = tempfile::NamedTempFile::new_in(&assets)?;
        std::io::Write::write_all(&mut tmp, bytes)?;
        tmp.persist(&dest).map_err(|e| e.error)?;
    }
    Ok(format!("{ASSETS_DIR}/{name}"))
}

/// `![alt](target)`: `target` in `<…>` form when it holds spaces or parentheses, brackets in
/// `alt` escaped, so any file name gives a valid link.
pub fn image_markdown(alt: &str, target: &str) -> String {
    let alt: String = alt.chars().flat_map(|c| if matches!(c, '[' | ']' | '\\') { vec!['\\', c] } else { vec![c] }).collect();
    let needs_angle = target.chars().any(|c| c.is_whitespace() || matches!(c, '(' | ')' | '<' | '>'));
    if needs_angle {
        let t = target.replace('<', "%3C").replace('>', "%3E");
        format!("![{alt}](<{t}>)")
    } else {
        format!("![{alt}]({target})")
    }
}

/// A line that holds nothing but one image: `(alt, target)`. Accepts `<…>` targets and an
/// optional title (`![a](b.png "T")`).
pub fn parse_image_line(line: &str) -> Option<(String, String)> {
    let rest = line.trim().strip_prefix("![")?;
    let (alt, after) = split_alt(rest)?;
    let inner = after.strip_prefix('(')?.strip_suffix(')')?;
    let (target, tail) = split_target(inner)?;
    let tail = tail.trim();
    let title_ok = tail.is_empty()
        || (tail.len() >= 2 && ((tail.starts_with('"') && tail.ends_with('"')) || (tail.starts_with('\'') && tail.ends_with('\''))));
    (title_ok && !target.is_empty()).then_some((alt, target))
}

/// Every image target in `text` (`![…](target)`), in order, outside fenced code.
pub fn image_targets(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_code = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        let mut s = line;
        while let Some(i) = s.find("![") {
            s = &s[i + 2..];
            let Some((_, after)) = split_alt(s) else { continue };
            let Some(inner) = after.strip_prefix('(') else { continue };
            let Some((target, _)) = split_target(inner) else { continue };
            if !target.is_empty() {
                out.push(target);
            }
        }
    }
    out
}

/// A link target as a path relative to the note: `None` for URLs (`https:`, `data:` …) and
/// absolute paths. Percent escapes are decoded (`a%20b.png` → `a b.png`).
pub fn local_target(target: &str) -> Option<PathBuf> {
    let t = target.trim();
    if t.contains("://") || t.starts_with("data:") || t.starts_with('/') || t.starts_with('#') || t.is_empty() {
        return None;
    }
    let t = t.split(['?', '#']).next().unwrap_or(t);
    let decoded = percent_decode(t);
    Some(PathBuf::from(decoded.trim_start_matches("./")))
}

/// Inserts `snippet` at the end of the note body: before the transcript section of a recording,
/// else at the end of the text. Separated from its neighbours by blank lines.
pub fn append_to_body(text: &str, snippet: &str) -> String {
    let snippet = snippet.trim_matches('\n');
    let marker = text.find("\n<!-- ln:transcript -->").map(|i| i + 1).or_else(|| text.starts_with("<!-- ln:transcript -->").then_some(0));
    match marker {
        Some(at) => {
            let (head, tail) = text.split_at(at);
            let head = head.trim_end_matches('\n');
            if head.is_empty() { format!("{snippet}\n\n{tail}") } else { format!("{head}\n\n{snippet}\n\n{tail}") }
        }
        None => {
            let head = text.trim_end_matches('\n');
            if head.is_empty() { format!("{snippet}\n") } else { format!("{head}\n\n{snippet}\n") }
        }
    }
}

/// `alt` up to the closing `]` (escapes respected), and the rest after it.
fn split_alt(s: &str) -> Option<(String, &str)> {
    let mut alt = String::new();
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                if let Some((_, n)) = chars.next() {
                    alt.push(n);
                }
            }
            ']' => return Some((alt, &s[i + 1..])),
            _ => alt.push(c),
        }
    }
    None
}

/// Target of `(target "title")` (without the opening parenthesis) and what follows it inside.
fn split_target(inner: &str) -> Option<(String, &str)> {
    let inner = inner.trim_start();
    if let Some(r) = inner.strip_prefix('<') {
        let end = r.find('>')?;
        return Some((r[..end].to_string(), &r[end + 1..]));
    }
    // Plain target: up to whitespace or the closing parenthesis (balanced pairs allowed).
    let mut depth = 0usize;
    for (i, c) in inner.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => return Some((inner[..i].to_string(), &inner[i..])),
            ')' => depth -= 1,
            c if c.is_whitespace() => return Some((inner[..i].to_string(), &inner[i..])),
            _ => {}
        }
    }
    Some((inner.to_string(), ""))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

fn hex(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_by_content_hash_once() {
        let d = tempfile::tempdir().unwrap();
        let a = store_asset(d.path(), b"png-bytes", "PNG").unwrap();
        let b = store_asset(d.path(), b"png-bytes", "png").unwrap();
        assert_eq!(a, b);
        assert!(a.starts_with("assets/") && a.ends_with(".png"));
        assert_eq!(a.len(), "assets/".len() + 12 + 4);
        assert_eq!(std::fs::read_dir(d.path().join("assets")).unwrap().count(), 1);
        let c = store_asset(d.path(), b"other", "png").unwrap();
        assert_ne!(a, c);
    }

    #[test]
    fn links_are_valid_for_any_name() {
        assert_eq!(image_markdown("Shot", "assets/ab.png"), "![Shot](assets/ab.png)");
        assert_eq!(image_markdown("a [b]", "assets/my pic (1).png"), "![a \\[b\\]](<assets/my pic (1).png>)");
        // Round trip through the parser.
        for (alt, target) in [("Shot", "assets/ab.png"), ("a [b]", "assets/my pic (1).png")] {
            let md = image_markdown(alt, target);
            assert_eq!(parse_image_line(&md), Some((alt.to_string(), target.to_string())), "{md}");
            assert_eq!(image_targets(&format!("x {md} y")), vec![target.to_string()]);
        }
    }

    #[test]
    fn parses_image_lines() {
        assert_eq!(parse_image_line("  ![a](b.png \"Title\")  "), Some(("a".into(), "b.png".into())));
        assert_eq!(parse_image_line("![](b.png)"), Some(("".into(), "b.png".into())));
        assert_eq!(parse_image_line("text ![a](b.png)"), None);
        assert_eq!(parse_image_line("![a](b.png) text"), None);
        assert_eq!(parse_image_line("[a](b.png)"), None);
    }

    #[test]
    fn collects_targets_outside_code() {
        let t = "![a](assets/1.png) and ![b](<assets/2 x.png>)\n```\n![c](assets/3.png)\n```\n![d](https://x/y.png)\n";
        assert_eq!(image_targets(t), vec!["assets/1.png", "assets/2 x.png", "https://x/y.png"]);
    }

    #[test]
    fn local_targets() {
        assert_eq!(local_target("assets/a%20b.png"), Some(PathBuf::from("assets/a b.png")));
        assert_eq!(local_target("./assets/a.png"), Some(PathBuf::from("assets/a.png")));
        assert_eq!(local_target("https://x/a.png"), None);
        assert_eq!(local_target("data:image/png;base64,xx"), None);
        assert_eq!(local_target("/abs/a.png"), None);
    }

    #[test]
    fn appends_before_transcript() {
        assert_eq!(append_to_body("# A\n\ntext\n", "![x](assets/x.png)"), "# A\n\ntext\n\n![x](assets/x.png)\n");
        assert_eq!(append_to_body("", "![x](y)"), "![x](y)\n");
        let rec = "<!-- ln:notes -->\n## Notizen\n- a\n\n<!-- ln:transcript -->\n## Transkript\n[00:00:01] Ich (de): Hallo\n";
        assert_eq!(
            append_to_body(rec, "![x](y)\n![z](w)\n"),
            "<!-- ln:notes -->\n## Notizen\n- a\n\n![x](y)\n![z](w)\n\n<!-- ln:transcript -->\n## Transkript\n[00:00:01] Ich (de): Hallo\n"
        );
    }

    #[test]
    fn image_extensions() {
        assert!(is_image_path(Path::new("/a/B.JPG")));
        assert!(is_image_path(Path::new("x.svg")));
        assert!(!is_image_path(Path::new("x.pdf")));
        assert!(!is_image_path(Path::new("png")));
    }
}
