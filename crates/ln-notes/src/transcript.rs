use ln_types::{Channel, Lang, Segment, UiLanguage};
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::LazyLock;

/// One line of the `Transcript` section: `[HH:MM:SS] <Label> (<lang>): <Text>`.
///
/// A line may link its timestamp, e.g. to the moment in a YouTube video:
/// `[HH:MM:SS](https://…&t=62s) <Label> (<lang>): <Text>` — a plain Markdown link elsewhere.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptLine {
    pub at_ms: u64,
    /// Target of the timestamp link, if any.
    pub link: Option<String>,
    /// Channel label ("Ich", "Others") or a speaker name from the front matter.
    pub label: String,
    pub lang: Lang,
    pub text: String,
}

static LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[(\d{1,3}):(\d{2}):(\d{2})\](?:\((https?://[^\s()]+)\))? (.+?) \(([a-z]{2,3}(?:-[a-z]{2,4})?)\): ?(.*)$").unwrap()
});

impl TranscriptLine {
    pub fn parse(line: &str) -> Option<TranscriptLine> {
        let c = LINE.captures(line.trim_end())?;
        let h: u64 = c[1].parse().ok()?;
        let m: u64 = c[2].parse().ok()?;
        let s: u64 = c[3].parse().ok()?;
        Some(TranscriptLine {
            at_ms: ((h * 60 + m) * 60 + s) * 1000,
            link: c.get(4).map(|m| m.as_str().to_string()),
            label: c[5].to_string(),
            lang: Lang::new(&c[6]),
            text: c[7].to_string(),
        })
    }

    pub fn format(&self) -> String {
        let link = self.link.as_deref().map(|l| format!("({l})")).unwrap_or_default();
        format!("[{}]{link} {} ({}): {}", format_timestamp(self.at_ms), self.label, self.lang, self.text.trim())
    }

    /// Builds a line from a segment, using a speaker name when one is set for the channel.
    pub fn from_segment(seg: &Segment, ui: UiLanguage, speakers: &BTreeMap<String, String>) -> Self {
        let label = speakers.get(seg.channel.id()).cloned().unwrap_or_else(|| seg.channel.label(ui).to_string());
        TranscriptLine { at_ms: seg.start_ms, link: None, label, lang: seg.lang.clone(), text: seg.text.clone() }
    }

    /// Resolves the channel of a line: either a channel label or a renamed speaker.
    pub fn channel(&self, speakers: &BTreeMap<String, String>) -> Option<Channel> {
        Channel::from_label(&self.label)
            .or_else(|| speakers.iter().find(|(_, name)| **name == self.label).and_then(|(id, _)| Channel::from_label(id)))
    }
}

pub fn format_timestamp(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

pub fn parse_timestamp(s: &str) -> Option<u64> {
    let parts: Vec<u64> = s.split(':').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    match parts.as_slice() {
        [h, m, s] => Some(((h * 60 + m) * 60 + s) * 1000),
        [m, s] => Some((m * 60 + s) * 1000),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_format_roundtrip() {
        let l = "[00:12:03] Andere (zh): 我们下周再讨论预算。";
        let t = TranscriptLine::parse(l).unwrap();
        assert_eq!(t.at_ms, 723_000);
        assert_eq!(t.label, "Andere");
        assert_eq!(t.lang.as_str(), "zh");
        assert_eq!(t.format(), l);
        assert_eq!(t.channel(&BTreeMap::new()), Some(Channel::Others));
    }

    #[test]
    fn renamed_speaker_maps_to_channel() {
        let mut sp = BTreeMap::new();
        sp.insert("others".to_string(), "Frau Li".to_string());
        let t = TranscriptLine::parse("[01:00:09] Frau Li (zh): 大家好").unwrap();
        assert_eq!(t.channel(&sp), Some(Channel::Others));
        assert_eq!(t.at_ms, 3_609_000);
    }

    #[test]
    fn linked_timestamp_roundtrip() {
        let l = "[00:01:02](https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=62s) Rick Astley (en): Never gonna give you up";
        let t = TranscriptLine::parse(l).unwrap();
        assert_eq!(t.at_ms, 62_000);
        assert_eq!(t.link.as_deref(), Some("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=62s"));
        assert_eq!(t.label, "Rick Astley");
        assert_eq!(t.text, "Never gonna give you up");
        assert_eq!(t.format(), l);
        assert_eq!(TranscriptLine::parse("[00:00:01] Ich (de): Hallo").unwrap().link, None);
    }

    #[test]
    fn rejects_other_lines() {
        assert!(TranscriptLine::parse("Hello").is_none());
        assert!(TranscriptLine::parse("[00:00] Me (de): x").is_none());
    }
}
