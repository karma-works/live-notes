//! Reading view of a note (SPEC §5 F10): Markdown blocks, clickable task items,
//! transcript rows and a header line from the front matter — without changing the file.

use ln_notes::{Note, TranscriptLine};
use ln_types::Channel;

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// From the front matter: shown as one quiet line.
    Header {
        date: Option<String>,
        duration: Option<String>,
        languages: Vec<String>,
        labels: Vec<String>,
    },
    Markdown(String),
    /// An image on its own line (`![alt](path)`); `path` as written in the note, relative to it.
    Image {
        alt: String,
        path: String,
    },
    Task {
        line: usize,
        checked: bool,
        text: String,
        indent: usize,
    },
    TranscriptHeading(String),
    Transcript(Vec<(TranscriptLine, Option<Channel>)>),
}

fn task(line: &str) -> Option<(usize, bool, String)> {
    let indent = line.len() - line.trim_start().len();
    let rest = line.trim_start();
    let mut chars = rest.chars();
    let bullet = chars.next()?;
    if !matches!(bullet, '-' | '*' | '+') {
        return None;
    }
    let after = &rest[1..];
    if let Some(t) = after.strip_prefix(" [ ] ") {
        return Some((indent, false, t.to_string()));
    }
    after.strip_prefix(" [x] ").or_else(|| after.strip_prefix(" [X] ")).map(|t| (indent, true, t.to_string()))
}

pub fn blocks(text: &str) -> Vec<Block> {
    let note = Note::parse(text).unwrap_or_default();
    let speakers = note.speakers();
    let mut out = Vec::new();
    // Front matter that only holds categories (shown in the note's top bar) gives no line.
    if let Some(f) =
        note.front.as_ref().filter(|f| f.created.is_some() || f.duration.is_some() || !f.languages.is_empty() || !f.labels.is_empty())
    {
        out.push(Block::Header {
            date: f.created.map(|c| c.with_timezone(&chrono::Local).format("%d.%m.%Y %H:%M").to_string()),
            duration: f.duration.clone(),
            languages: f.languages.iter().map(|l| l.to_string()).collect(),
            labels: f.labels.clone(),
        });
    }
    // Walk the raw lines so task line numbers match the file.
    let mut lines = text.split_inclusive('\n').enumerate().peekable();
    // Skip front matter.
    if text.starts_with("---\n") {
        lines.next();
        for (_, l) in lines.by_ref() {
            if l.trim_end() == "---" {
                break;
            }
        }
    }
    let mut md = String::new();
    let mut in_code = false;
    let mut in_transcript = false;
    let mut transcript: Vec<(TranscriptLine, Option<Channel>)> = Vec::new();
    // Heading of a marked section, shown only once the section has content (SPEC §6.1).
    let mut pending_heading: Option<String> = None;
    let mut after_marker = false;
    let flush_md = |md: &mut String, out: &mut Vec<Block>| {
        if !md.trim().is_empty() {
            out.push(Block::Markdown(std::mem::take(md)));
        }
        md.clear();
    };
    for (i, raw) in lines {
        let line = raw.trim_end_matches(['\n', '\r']);
        if let Some(id) = line.strip_prefix("<!-- ln:").and_then(|r| r.strip_suffix(" -->")) {
            flush_md(&mut md, &mut out);
            if in_transcript && !transcript.is_empty() {
                out.push(Block::Transcript(std::mem::take(&mut transcript)));
            }
            in_transcript = id.trim() == "transcript";
            pending_heading = None;
            after_marker = true;
            continue;
        }
        if after_marker && !in_transcript && line.starts_with('#') {
            pending_heading = Some(raw.to_string());
            after_marker = false;
            continue;
        }
        after_marker = false;
        if !in_transcript
            && !line.trim().is_empty()
            && let Some(h) = pending_heading.take()
        {
            md.push_str(&h);
        }
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
        }
        if in_transcript && !in_code {
            if let Some(t) = TranscriptLine::parse(line) {
                let ch = t.channel(&speakers);
                transcript.push((t, ch));
                continue;
            }
            if line.starts_with('#') {
                out.push(Block::TranscriptHeading(line.trim_start_matches('#').trim().to_string()));
                continue;
            }
            if line.trim().is_empty() {
                continue;
            }
        }
        if !in_code
            && !in_transcript
            && let Some((alt, path)) = ln_notes::parse_image_line(line)
        {
            flush_md(&mut md, &mut out);
            out.push(Block::Image { alt, path });
            continue;
        }
        if !in_code && let Some((indent, checked, t)) = task(line) {
            flush_md(&mut md, &mut out);
            out.push(Block::Task { line: i, checked, text: t, indent });
            continue;
        }
        md.push_str(raw);
    }
    flush_md(&mut md, &mut out);
    if !transcript.is_empty() {
        out.push(Block::Transcript(transcript));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_note_into_blocks() {
        let t = "---\ntitle: A\nlanguages:\n- zh\n---\n\n<!-- ln:tasks -->\n## Aufgaben\n- [ ] Senden — Li\n- [x] Fertig\n\n<!-- ln:transcript -->\n## Transkript\n[00:00:01] Ich (de): Hallo\n[00:00:04] Andere (zh): 你好\n";
        let b = blocks(t);
        assert!(matches!(&b[0], Block::Header { languages, .. } if languages == &vec!["zh".to_string()]));
        assert!(b.iter().any(|x| matches!(x, Block::Task { line: 8, checked: false, .. })));
        assert!(b.iter().any(|x| matches!(x, Block::Task { line: 9, checked: true, .. })));
        let tr = b.iter().find_map(|x| if let Block::Transcript(v) = x { Some(v) } else { None }).unwrap();
        assert_eq!(tr.len(), 2);
        assert_eq!(tr[1].1, Some(Channel::Others));
        // The line numbers point at the real task lines.
        assert!(t.lines().nth(8).unwrap().contains("Senden"));
    }

    #[test]
    fn empty_sections_have_no_heading() {
        let b = blocks("<!-- ln:notes -->\n## Meine Notizen\n\n<!-- ln:summary -->\n## Zusammenfassung\n- A\n");
        assert_eq!(b, vec![Block::Markdown("## Zusammenfassung\n- A\n".into())]);
    }

    #[test]
    fn images_on_their_own_line_are_blocks() {
        let b = blocks("# Bilder\n\nVorher\n![Shot 1](assets/0123456789ab.png)\n![](<assets/a b.png> \"T\")\nNachher ![inline](x.png)\n");
        assert_eq!(
            b,
            vec![
                Block::Markdown("# Bilder\n\nVorher\n".into()),
                Block::Image { alt: "Shot 1".into(), path: "assets/0123456789ab.png".into() },
                Block::Image { alt: "".into(), path: "assets/a b.png".into() },
                Block::Markdown("Nachher ![inline](x.png)\n".into()),
            ]
        );
    }

    #[test]
    fn images_in_sections_keep_their_heading_and_code_stays_code() {
        let b = blocks("<!-- ln:notes -->\n## Meine Notizen\n![a](assets/a.png)\n```\n![b](assets/b.png)\n```\n");
        assert_eq!(b[0], Block::Markdown("## Meine Notizen\n".into()));
        assert_eq!(b[1], Block::Image { alt: "a".into(), path: "assets/a.png".into() });
        assert!(matches!(&b[2], Block::Markdown(m) if m.contains("![b](assets/b.png)")));
        assert_eq!(b.len(), 3);
    }

    #[test]
    fn code_blocks_keep_checkbox_text() {
        let b = blocks("# X\n```\n- [ ] not a task\n```\n");
        assert_eq!(b.len(), 1);
    }
}
