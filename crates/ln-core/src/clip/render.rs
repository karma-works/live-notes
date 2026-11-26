//! The Markdown note of a clipped page or video (`docs/clip-api.md`, "Ergebnis-Notiz").

use super::ClipKind;
use super::captions::CaptionCue;
use super::youtube::timestamp_url;
use chrono::{DateTime, FixedOffset};
use ln_ai::prompts::WebSummary;
use ln_notes::{FrontMatter, Note, Section, SectionKind, TranscriptLine, headings};
use ln_types::{Lang, UiLanguage};

pub(crate) enum Body {
    Page {
        text: String,
        /// The text is the user's selection, not the whole page.
        selection: bool,
    },
    Video {
        video_id: String,
        lines: Vec<CaptionCue>,
        /// Where the transcript comes from when the app made it (`youtube/captions`, `local/whisper-…`).
        transcriber: Option<String>,
    },
}

pub(crate) struct ClipNote {
    pub kind: ClipKind,
    pub url: String,
    pub title: String,
    pub site_name: Option<String>,
    pub byline: Option<String>,
    pub channel: Option<String>,
    pub duration_ms: Option<u64>,
    pub lang: Option<String>,
    pub created: DateTime<FixedOffset>,
    /// `Err`: why there is no summary (shown instead).
    pub summary: Result<WebSummary, String>,
    pub body: Body,
}

fn tr(ui: UiLanguage, de: &'static str, en: &'static str) -> &'static str {
    match ui {
        UiLanguage::De => de,
        UiLanguage::En => en,
    }
}

/// Text from outside must not create section markers or break the layout.
fn escape(s: &str) -> String {
    s.replace("<!-- ln:", "&lt;!-- ln:")
}

/// One line of outside text.
fn one_line(s: &str) -> String {
    escape(&s.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Text of a Markdown link: one line without brackets.
fn link_text(s: &str) -> String {
    one_line(s).replace(['[', ']'], "")
}

/// Parentheses would end a Markdown link target early.
fn link_url(url: &str) -> String {
    url.replace('(', "%28").replace(')', "%29")
}

pub(crate) fn host(url: &str) -> String {
    let rest = url.split_once("://").map(|x| x.1).unwrap_or(url);
    let h = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    h.strip_prefix("www.").unwrap_or(h).to_string()
}

pub(crate) fn render(n: &ClipNote, ui: UiLanguage) -> String {
    let front = FrontMatter {
        id: Some(ulid::Ulid::new().to_string()),
        title: Some(n.title.clone()),
        created: Some(n.created),
        duration: n.duration_ms.map(ln_notes::format_timestamp),
        source: Some(n.url.clone()),
        source_kind: Some(match n.kind {
            ClipKind::Page => "page".into(),
            ClipKind::Youtube => "youtube".into(),
        }),
        channel: n.channel.clone(),
        // Where the note was found, shown as `#Web #example.com` / `#YouTube` in the header line.
        labels: match n.kind {
            ClipKind::Page => vec!["Web".into(), host(&n.url)],
            ClipKind::Youtube => vec!["YouTube".into()],
        },
        languages: n.lang.iter().map(Lang::new).collect(),
        transcription: match &n.body {
            Body::Video { transcriber, .. } => transcriber.clone(),
            Body::Page { .. } => None,
        },
        app: Some(format!("live-notes/{}", env!("CARGO_PKG_VERSION"))),
        ..Default::default()
    };
    // Source line: link, site or channel, author, length.
    let mut meta = vec![match n.kind {
        ClipKind::Page => format!("[{}]({})", link_text(&host(&n.url)), link_url(&n.url)),
        ClipKind::Youtube => format!("[YouTube]({})", link_url(&n.url)),
    }];
    meta.extend(n.site_name.as_deref().map(one_line));
    meta.extend(n.channel.as_deref().map(one_line));
    meta.extend(n.byline.as_deref().map(one_line));
    meta.extend(n.duration_ms.filter(|d| *d > 0).map(|d| {
        let s = d / 1000;
        if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
    }));
    let preamble = format!("# {}\n\n{}\n", one_line(&n.title), meta.join(" · "));
    let mut note = Note { front: Some(front), preamble, sections: vec![] };

    let summary_body = match &n.summary {
        Ok(s) => {
            let mut b = format!("{}\n", escape(s.summary.trim()));
            if !s.key_points.is_empty() {
                b.push_str(&format!("\n### {}\n", tr(ui, "Kernpunkte", "Key points")));
                for k in &s.key_points {
                    b.push_str(&format!("- {}\n", one_line(k.trim_start_matches(['-', '*', ' ']))));
                }
            }
            b
        }
        Err(reason) => format!("> **{}** {}\n", tr(ui, "Keine Zusammenfassung:", "No summary:"), one_line(reason)),
    };
    note.sections.push(Section {
        kind: SectionKind::Summary,
        heading: format!("## {}", headings(&SectionKind::Summary, ui)),
        body: summary_body,
    });

    match &n.body {
        Body::Page { text, selection } => {
            let heading = if *selection { tr(ui, "Markierter Text", "Selected text") } else { tr(ui, "Seitentext", "Page text") };
            note.sections.push(Section {
                kind: SectionKind::Other("source".into()),
                heading: format!("## {heading}"),
                body: format!("{}\n", escape(text.trim())),
            });
        }
        Body::Video { video_id, lines, .. } => {
            let label = n.channel.as_deref().map(speaker_label).filter(|l| !l.is_empty()).unwrap_or_else(|| "YouTube".into());
            let lang = n.lang.as_deref().map(Lang::new).unwrap_or_else(|| Lang::new("und"));
            let mut body = String::new();
            for c in lines {
                let line = TranscriptLine {
                    at_ms: c.start_ms,
                    link: Some(timestamp_url(video_id, c.start_ms)),
                    label: label.clone(),
                    lang: lang.clone(),
                    text: escape(&c.text.split_whitespace().collect::<Vec<_>>().join(" ")),
                };
                body.push_str(&line.format());
                body.push('\n');
            }
            if body.is_empty() {
                body = format!("{}\n", tr(ui, "(kein Transkript)", "(no transcript)"));
            }
            note.sections.push(Section {
                kind: SectionKind::Transcript,
                heading: format!("## {}", headings(&SectionKind::Transcript, ui)),
                body,
            });
        }
    }
    note.render()
}

/// A transcript label must not contain the characters the line format uses.
fn speaker_label(s: &str) -> String {
    let l: String = s.chars().filter(|c| !matches!(c, '(' | ')' | '[' | ']' | ':')).collect();
    l.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Paragraphs separated by one blank line, lines trimmed.
pub(crate) fn clean_page_text(text: &str) -> String {
    let mut out = String::new();
    let mut blank = false;
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() {
            blank = !out.is_empty();
            continue;
        }
        if blank {
            out.push_str("\n\n");
            blank = false;
        } else if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(l);
    }
    out
}

/// Readable text from Readability's HTML, for clips that only send `html`: block elements become
/// paragraphs, everything else is dropped.
pub(crate) fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut tag = String::new();
    let (mut in_tag, mut skip) = (false, 0usize);
    for c in html.chars() {
        if in_tag {
            if c == '>' {
                in_tag = false;
                let t = tag.trim_start_matches('/').split([' ', '\t', '\n', '/']).next().unwrap_or("").to_ascii_lowercase();
                let closing = tag.starts_with('/');
                if matches!(t.as_str(), "script" | "style" | "noscript") {
                    skip = if closing { skip.saturating_sub(1) } else { skip + 1 };
                }
                if matches!(
                    t.as_str(),
                    "p" | "div"
                        | "br"
                        | "li"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "tr"
                        | "blockquote"
                        | "pre"
                        | "section"
                        | "article"
                ) {
                    out.push_str("\n\n");
                }
                tag.clear();
            } else {
                tag.push(c);
            }
        } else if c == '<' {
            in_tag = true;
        } else if skip == 0 {
            out.push(c);
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    clean_page_text(&decoded.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join("\n"))
}

/// Splits text into parts of at most `max` characters, at paragraph or line ends where possible.
pub(crate) fn split_text(text: &str, max: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0usize;
    for line in text.split_inclusive('\n') {
        let n = line.chars().count();
        if cur_len + n > max && !cur.is_empty() {
            parts.push(std::mem::take(&mut cur));
            cur_len = 0;
        }
        if n > max {
            // A single huge line: cut it by characters.
            let chars: Vec<char> = line.chars().collect();
            for piece in chars.chunks(max) {
                parts.push(piece.iter().collect());
            }
            continue;
        }
        cur.push_str(line);
        cur_len += n;
    }
    if !cur.trim().is_empty() {
        parts.push(cur);
    }
    parts
}
