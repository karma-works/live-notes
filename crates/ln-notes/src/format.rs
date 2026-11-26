use crate::{Error, Result, TranscriptLine};
use chrono::{DateTime, FixedOffset};
use ln_types::{Lang, Segment, UiLanguage};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// YAML front matter of a note. Unknown keys are kept so external tools can add their own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct FrontMatter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<DateTime<FixedOffset>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<String>,
    /// Where a clipped note comes from (URL of the web page or video).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// `page` or `youtube` for clipped notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_kind: Option<String>,
    /// YouTube channel of a clipped video.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<Lang>,
    /// Which transcriber produced the transcript, e.g. `local/whisper-large-v3-turbo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcription: Option<String>,
    /// Channel id → speaker name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub speakers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// The user's categories this note belongs to, set by the decision model or by hand.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    /// Set when the user chose the categories by hand; automatic sorting then leaves them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub categories_manual: bool,
    /// Set when the user renamed the file; the app then never renames it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub manual_name: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml_ng::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionKind {
    Summary,
    Decisions,
    Tasks,
    Notes,
    Suggestions,
    Transcript,
    Other(String),
}

impl SectionKind {
    pub fn marker_id(&self) -> &str {
        match self {
            SectionKind::Summary => "summary",
            SectionKind::Decisions => "decisions",
            SectionKind::Tasks => "tasks",
            SectionKind::Notes => "notes",
            SectionKind::Suggestions => "suggestions",
            SectionKind::Transcript => "transcript",
            SectionKind::Other(s) => s,
        }
    }

    fn from_marker(id: &str) -> SectionKind {
        match id {
            "summary" => SectionKind::Summary,
            "decisions" => SectionKind::Decisions,
            "tasks" => SectionKind::Tasks,
            "notes" => SectionKind::Notes,
            "suggestions" => SectionKind::Suggestions,
            "transcript" => SectionKind::Transcript,
            other => SectionKind::Other(other.to_string()),
        }
    }
}

/// Localised section headings, written when the note is created (SPEC §8.1).
pub fn headings(kind: &SectionKind, ui: UiLanguage) -> &'static str {
    match (kind, ui) {
        (SectionKind::Summary, UiLanguage::De) => "Zusammenfassung",
        (SectionKind::Summary, UiLanguage::En) => "Summary",
        (SectionKind::Decisions, UiLanguage::De) => "Entscheidungen",
        (SectionKind::Decisions, UiLanguage::En) => "Decisions",
        (SectionKind::Tasks, UiLanguage::De) => "Aufgaben",
        (SectionKind::Tasks, UiLanguage::En) => "Tasks",
        (SectionKind::Notes, UiLanguage::De) => "Meine Notizen",
        (SectionKind::Notes, UiLanguage::En) => "My notes",
        (SectionKind::Suggestions, UiLanguage::De) => "Während des Gesprächs",
        (SectionKind::Suggestions, UiLanguage::En) => "During the conversation",
        (SectionKind::Transcript, UiLanguage::De) => "Transkript",
        (SectionKind::Transcript, UiLanguage::En) => "Transcript",
        (SectionKind::Other(_), _) => "",
    }
}

/// A marked section. `body` is everything after the heading line up to the next marker.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub kind: SectionKind,
    /// The heading line as written in the file, e.g. `## Transkript`.
    pub heading: String,
    pub body: String,
}

/// A parsed note. Parsing and rendering roundtrip byte-for-byte for files the app wrote.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Note {
    pub front: Option<FrontMatter>,
    /// Text before the first marker (the whole body for manual notes).
    pub preamble: String,
    pub sections: Vec<Section>,
}

const MARKER_PREFIX: &str = "<!-- ln:";
const MARKER_SUFFIX: &str = " -->";

impl Note {
    pub fn parse(text: &str) -> Result<Note> {
        let (front, body) = split_front_matter(text)?;
        let mut note = Note { front, preamble: String::new(), sections: vec![] };
        let mut current: Option<(SectionKind, Vec<&str>)> = None;
        let mut pre = Vec::new();
        for line in body.split_inclusive('\n') {
            let trimmed = line.trim_end();
            if let Some(id) = trimmed.strip_prefix(MARKER_PREFIX).and_then(|r| r.strip_suffix(MARKER_SUFFIX)) {
                if let Some((kind, lines)) = current.take() {
                    note.sections.push(make_section(kind, &lines));
                }
                current = Some((SectionKind::from_marker(id.trim()), Vec::new()));
                continue;
            }
            match current.as_mut() {
                Some((_, lines)) => lines.push(line),
                None => pre.push(line),
            }
        }
        if let Some((kind, lines)) = current.take() {
            note.sections.push(make_section(kind, &lines));
        }
        note.preamble = pre.concat();
        Ok(note)
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        if let Some(f) = &self.front {
            out.push_str("---\n");
            let yaml = serde_yaml_ng::to_string(f).unwrap_or_default();
            out.push_str(&yaml);
            if !yaml.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("---\n");
        }
        out.push_str(&self.preamble);
        for s in &self.sections {
            if !out.is_empty() && !out.ends_with("\n\n") {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push('\n');
            }
            out.push_str(MARKER_PREFIX);
            out.push_str(s.kind.marker_id());
            out.push_str(MARKER_SUFFIX);
            out.push('\n');
            if !s.heading.is_empty() {
                out.push_str(&s.heading);
                out.push('\n');
            }
            out.push_str(&s.body);
        }
        out
    }

    pub fn section(&self, kind: &SectionKind) -> Option<&Section> {
        self.sections.iter().find(|s| &s.kind == kind)
    }

    pub fn section_mut(&mut self, kind: &SectionKind) -> Option<&mut Section> {
        self.sections.iter_mut().find(|s| &s.kind == kind)
    }

    /// Sets the body of a section, creating it (with a localised heading) if missing.
    /// New sections are inserted before the transcript so it stays last.
    pub fn set_section(&mut self, kind: SectionKind, body: &str, ui: UiLanguage) {
        let mut body = body.trim_end().to_string();
        body.push('\n');
        if let Some(s) = self.section_mut(&kind) {
            s.body = body;
            return;
        }
        let heading = format!("## {}", headings(&kind, ui));
        let sec = Section { kind, heading, body };
        let pos = self.sections.iter().position(|s| s.kind == SectionKind::Transcript).unwrap_or(self.sections.len());
        self.sections.insert(pos, sec);
    }

    pub fn title(&self) -> Option<String> {
        if let Some(t) = self.front.as_ref().and_then(|f| f.title.clone()) {
            return Some(t);
        }
        first_heading(&self.preamble)
    }

    pub fn categories(&self) -> Vec<String> {
        self.front.as_ref().map(|f| f.categories.clone()).unwrap_or_default()
    }

    /// Sets the categories. A note without front matter only gets one when there is
    /// something to store, so plain Markdown notes stay plain.
    pub fn set_categories(&mut self, categories: Vec<String>) {
        match self.front.as_mut() {
            Some(f) => f.categories = categories,
            None if categories.is_empty() => {}
            None => self.front = Some(FrontMatter { categories, ..Default::default() }),
        }
    }

    /// True when the user chose the categories by hand.
    pub fn categories_manual(&self) -> bool {
        self.front.as_ref().is_some_and(|f| f.categories_manual)
    }

    /// Sets the categories chosen by hand (or, with `manual = false`, hands them back to
    /// automatic sorting).
    pub fn set_categories_manual(&mut self, categories: Vec<String>, manual: bool) {
        self.set_categories(categories);
        if manual && self.front.is_none() {
            self.front = Some(FrontMatter::default());
        }
        if let Some(f) = self.front.as_mut() {
            f.categories_manual = manual;
        }
    }

    /// Renames the note as shown in the note list: the front-matter title if there is one,
    /// otherwise the first `# ` heading (added at the top when missing).
    pub fn set_title(&mut self, title: &str) {
        let title = title.trim();
        if let Some(f) = self.front.as_mut()
            && (f.title.is_some() || f.id.is_some() || (first_heading(&self.preamble).is_none() && !self.sections.is_empty()))
        {
            f.title = Some(title.to_string());
            f.manual_name = true;
            return;
        }
        let mut done = false;
        let mut out = String::with_capacity(self.preamble.len() + title.len() + 3);
        for line in self.preamble.split_inclusive('\n') {
            if !done && (line.trim_start().starts_with("# ") || line.trim_end() == "#") {
                let nl = if line.ends_with('\n') { "\n" } else { "" };
                out.push_str(&format!("# {title}{nl}"));
                done = true;
            } else {
                out.push_str(line);
            }
        }
        if !done {
            out = format!("# {title}\n\n{}", self.preamble.trim_start_matches('\n'));
        }
        self.preamble = out;
    }

    pub fn speakers(&self) -> BTreeMap<String, String> {
        self.front.as_ref().map(|f| f.speakers.clone()).unwrap_or_default()
    }

    pub fn transcript_lines(&self) -> Vec<TranscriptLine> {
        self.section(&SectionKind::Transcript).map(|s| s.body.lines().filter_map(TranscriptLine::parse).collect()).unwrap_or_default()
    }

    /// A new recording note with empty sections in the UI language (SPEC §8.1).
    pub fn new_recording(id: String, created: DateTime<FixedOffset>, transcriber: &str, ui: UiLanguage) -> Note {
        let front = FrontMatter {
            id: Some(id),
            created: Some(created),
            transcription: Some(transcriber.to_string()),
            app: Some(format!("live-notes/{}", env!("CARGO_PKG_VERSION"))),
            ..Default::default()
        };
        let mut note = Note { front: Some(front), preamble: "\n".into(), sections: vec![] };
        for kind in [SectionKind::Notes, SectionKind::Transcript] {
            note.sections.push(Section { heading: format!("## {}", headings(&kind, ui)), kind, body: "\n".into() });
        }
        note
    }

    /// Replaces the transcript with the given segments.
    pub fn set_transcript(&mut self, segments: &[Segment], ui: UiLanguage) {
        let speakers = self.speakers();
        let mut body = String::new();
        for seg in segments {
            body.push_str(&TranscriptLine::from_segment(seg, ui, &speakers).format());
            body.push('\n');
        }
        let mut langs: Vec<Lang> = Vec::new();
        // Languages ordered by how much was said in them.
        let mut by_lang: BTreeMap<Lang, u64> = BTreeMap::new();
        for s in segments {
            *by_lang.entry(s.lang.clone()).or_default() += s.duration_ms().max(1);
        }
        let mut v: Vec<_> = by_lang.into_iter().collect();
        v.sort_by_key(|x| std::cmp::Reverse(x.1));
        langs.extend(v.into_iter().map(|(l, _)| l));
        if let Some(f) = self.front.as_mut() {
            f.languages = langs;
        }
        if let Some(s) = self.section_mut(&SectionKind::Transcript) {
            s.body = if body.is_empty() { "\n".into() } else { body };
        } else {
            self.set_section(SectionKind::Transcript, &body, ui);
        }
    }

    /// The main language of a recording: most spoken, first in `languages`.
    pub fn main_language(&self) -> Option<Lang> {
        self.front.as_ref().and_then(|f| f.languages.first().cloned())
    }
}

fn make_section(kind: SectionKind, lines: &[&str]) -> Section {
    // The heading is the first non-empty line if it is an ATX heading.
    let mut iter = lines.iter().peekable();
    let mut heading = String::new();
    if let Some(first) = iter.peek()
        && first.trim_start().starts_with('#')
    {
        heading = first.trim_end_matches(['\n', '\r']).to_string();
        iter.next();
    }
    Section { kind, heading, body: iter.copied().collect::<String>() }
}

fn split_front_matter(text: &str) -> Result<(Option<FrontMatter>, &str)> {
    // Blank lines typed above the front matter must not hide it: the note would lose its date
    // (and sort by modification time), and categories would get a second front matter on top.
    let start = text.trim_start_matches(['\n', '\r', ' ', '\t']);
    let Some(rest) = start.strip_prefix("---\n").or_else(|| start.strip_prefix("---\r\n")) else {
        return Ok((None, text));
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            let yaml = &rest[..offset];
            let body = &rest[offset + line.len()..];
            let front: FrontMatter = if yaml.trim().is_empty() {
                FrontMatter::default()
            } else {
                serde_yaml_ng::from_str(yaml).map_err(|e| Error::FrontMatter(e.to_string()))?
            };
            return Ok((Some(front), body));
        }
        offset += line.len();
    }
    // No closing fence: treat as plain text.
    Ok((None, text))
}

fn first_heading(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let t = l.trim_start();
        t.strip_prefix("# ").map(|h| h.trim().to_string()).filter(|h| !h.is_empty())
    })
}

/// Toggles the task checkbox on a 0-based line of the full file text. Returns the new text,
/// or `None` if that line is not a task list item.
pub fn toggle_task(text: &str, line_idx: usize) -> Option<String> {
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
    let line = lines.get_mut(line_idx)?;
    let indent = line.len() - line.trim_start().len();
    let rest = &line[indent..];
    let bullet = rest.chars().next()?;
    if !matches!(bullet, '-' | '*' | '+') {
        return None;
    }
    let after = &rest[1..];
    let new = if let Some(r) = after.strip_prefix(" [ ]") {
        format!("{}{} [x]{}", &line[..indent], bullet, r)
    } else {
        let r = after.strip_prefix(" [x]").or_else(|| after.strip_prefix(" [X]"))?;
        format!("{}{} [ ]{}", &line[..indent], bullet, r)
    };
    *line = new;
    Some(lines.concat())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ln_types::Channel;

    const SAMPLE: &str = "---\nid: 01JB8Z4K7Q2W3E4R5T6Y7U8I9O\ntitle: Teamrunde Budget\nlanguages:\n- zh\n- de\nspeakers:\n  others: Frau Li\n---\n\n<!-- ln:summary -->\n## Zusammenfassung\nBudget nächste Woche.\n\n<!-- ln:notes -->\n## Meine Notizen\n- Budget Q1 klären\n\n<!-- ln:transcript -->\n## Transkript\n[00:00:03] Ich (de): Hallo zusammen\n[00:00:09] Frau Li (zh): 大家好\n";

    #[test]
    fn parses_sections_and_roundtrips() {
        let n = Note::parse(SAMPLE).unwrap();
        assert_eq!(n.title().unwrap(), "Teamrunde Budget");
        assert_eq!(n.sections.len(), 3);
        assert_eq!(n.section(&SectionKind::Notes).unwrap().body, "- Budget Q1 klären\n\n");
        let lines = n.transcript_lines();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].channel(&n.speakers()), Some(Channel::Others));
        let again = Note::parse(&n.render()).unwrap();
        assert_eq!(again, n);
    }

    #[test]
    fn blank_lines_above_front_matter_do_not_hide_it() {
        let n = Note::parse(&format!("\n\n{SAMPLE}")).unwrap();
        assert_eq!(n.title().unwrap(), "Teamrunde Budget");
        assert!(n.render().starts_with("---\n"), "the blank lines go away on the next save");
        assert_eq!(Note::parse("\n\n---\nkein Front Matter").unwrap().front, None);
    }

    #[test]
    fn manual_note_without_front_matter() {
        let n = Note::parse("# Einkauf\n\n- [ ] Milch\n").unwrap();
        assert!(n.front.is_none());
        assert_eq!(n.title().unwrap(), "Einkauf");
        assert_eq!(n.render(), "# Einkauf\n\n- [ ] Milch\n");
    }

    #[test]
    fn new_recording_and_transcript() {
        let created = DateTime::parse_from_rfc3339("2026-10-21T14:30:12+02:00").unwrap();
        let mut n = Note::new_recording("X".into(), created, "local/whisper", UiLanguage::De);
        let segs = vec![
            Segment { channel: Channel::Me, start_ms: 3000, end_ms: 5000, lang: "de".into(), text: "Hallo".into(), lang_confidence: 0.9 },
            Segment {
                channel: Channel::Others,
                start_ms: 9000,
                end_ms: 20000,
                lang: "zh".into(),
                text: "大家好".into(),
                lang_confidence: 0.9,
            },
        ];
        n.set_transcript(&segs, UiLanguage::De);
        n.set_section(SectionKind::Summary, "Kurz.", UiLanguage::De);
        let text = n.render();
        assert!(text.contains("<!-- ln:transcript -->\n## Transkript\n[00:00:03] Ich (de): Hallo\n"));
        assert!(text.contains("[00:00:09] Andere (zh): 大家好"));
        let back = Note::parse(&text).unwrap();
        assert_eq!(back.main_language().unwrap().as_str(), "zh");
        // Summary was inserted before the transcript.
        assert_eq!(back.sections.last().unwrap().kind, SectionKind::Transcript);
        assert!(back.section(&SectionKind::Summary).unwrap().body.starts_with("Kurz."));
    }

    #[test]
    fn manual_categories_survive_a_roundtrip() {
        let mut n = Note::parse("# Vokabeln\n").unwrap();
        n.set_categories_manual(vec!["Kurs".into()], true);
        let back = Note::parse(&n.render()).unwrap();
        assert!(back.categories_manual());
        assert_eq!(back.categories(), vec!["Kurs".to_string()]);
        // Cleared by hand: stays manual, so automatic sorting does not refill it.
        let mut cleared = back.clone();
        cleared.set_categories_manual(vec![], true);
        assert!(Note::parse(&cleared.render()).unwrap().categories_manual());
        let mut auto = back;
        auto.set_categories_manual(vec!["Kurs".into()], false);
        assert!(!auto.render().contains("categories_manual"));
    }

    #[test]
    fn categories_only_add_front_matter_when_needed() {
        let mut n = Note::parse("# Vokabeln\n\n- 你好\n").unwrap();
        n.set_categories(vec![]);
        assert_eq!(n.render(), "# Vokabeln\n\n- 你好\n");
        n.set_categories(vec!["Chinesisch-Kurs".into()]);
        let text = n.render();
        assert!(text.starts_with("---\ncategories:\n- Chinesisch-Kurs\n---\n# Vokabeln\n"), "{text}");
        let back = Note::parse(&text).unwrap();
        assert_eq!(back.categories(), vec!["Chinesisch-Kurs".to_string()]);
        assert_eq!(back.title().unwrap(), "Vokabeln");
        let mut rec = Note::parse(SAMPLE).unwrap();
        rec.set_categories(vec!["Arztagent".into(), "KI-Projekte".into()]);
        assert_eq!(Note::parse(&rec.render()).unwrap().categories().len(), 2);
    }

    #[test]
    fn set_title_on_manual_and_recording_notes() {
        let mut n = Note::parse("Intro\n# Alt\n\nText\n").unwrap();
        n.set_title("Neu");
        assert_eq!(n.render(), "Intro\n# Neu\n\nText\n");
        let mut empty = Note::parse("# \n").unwrap();
        empty.set_title("Einkauf");
        assert_eq!(empty.render(), "# Einkauf\n");
        let mut plain = Note::parse("Nur Text\n").unwrap();
        plain.set_title("Titel");
        assert_eq!(plain.render(), "# Titel\n\nNur Text\n");
        let mut tagged = Note::parse("---\ncategories:\n- A\n---\n# Alt\n").unwrap();
        tagged.set_title("Neu");
        assert!(tagged.render().ends_with("# Neu\n"));
        assert!(tagged.front.as_ref().unwrap().title.is_none());
        let mut rec = Note::parse(SAMPLE).unwrap();
        rec.set_title("Budgetrunde mit Frau Li");
        let back = Note::parse(&rec.render()).unwrap();
        assert_eq!(back.title().unwrap(), "Budgetrunde mit Frau Li");
        assert!(back.front.unwrap().manual_name);
    }

    #[test]
    fn toggles_tasks() {
        let t = "# A\n- [ ] eins\n  * [x] zwei\nText\n";
        let t1 = toggle_task(t, 1).unwrap();
        assert!(t1.contains("- [x] eins"));
        let t2 = toggle_task(&t1, 2).unwrap();
        assert!(t2.contains("  * [ ] zwei"));
        assert!(toggle_task(t, 3).is_none());
    }
}
