//! Captions: YouTube's json3 and WebVTT files, rolling auto-caption duplicates, and merging
//! short cues into transcript lines of readable length.

use serde::{Deserialize, Serialize};

/// One caption cue, as sent by the extension (`youtube.transcript[]`) or read from a file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptionCue {
    pub start_ms: u64,
    #[serde(default)]
    pub dur_ms: u64,
    pub text: String,
}

/// YouTube's `json3` caption format (`--sub-format json3`).
pub fn parse_json3(text: &str) -> Option<Vec<CaptionCue>> {
    #[derive(Deserialize)]
    struct File {
        #[serde(default)]
        events: Vec<Event>,
    }
    #[derive(Deserialize)]
    struct Event {
        #[serde(rename = "tStartMs", default)]
        start: u64,
        #[serde(rename = "dDurationMs", default)]
        dur: u64,
        #[serde(default)]
        segs: Vec<Seg>,
    }
    #[derive(Deserialize)]
    struct Seg {
        #[serde(default)]
        utf8: String,
    }
    let f: File = serde_json::from_str(text).ok()?;
    let cues = f
        .events
        .into_iter()
        .filter_map(|e| {
            let t: String = e.segs.iter().map(|s| s.utf8.as_str()).collect();
            let t = clean_text(&t);
            (!t.is_empty()).then_some(CaptionCue { start_ms: e.start, dur_ms: e.dur, text: t })
        })
        .collect();
    Some(dedupe(cues))
}

/// WebVTT, including YouTube's auto captions where every cue repeats the previous line and
/// carries word timings (`<00:00:01.230><c> word</c>`).
pub fn parse_vtt(text: &str) -> Vec<CaptionCue> {
    let mut cues = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some((start, end)) = parse_cue_timing(line) else { continue };
        let mut body = Vec::new();
        // A cue ends at an empty line; YouTube puts lines with a single space inside cues.
        while let Some(l) = lines.peek() {
            if l.trim_end_matches('\r').is_empty() {
                break;
            }
            body.push(lines.next().unwrap_or_default());
        }
        // Each line on its own: rolling captions repeat the earlier line above the new one.
        for l in body {
            let t = clean_text(&strip_tags(l));
            if !t.is_empty() {
                cues.push(CaptionCue { start_ms: start, dur_ms: end.saturating_sub(start), text: t });
            }
        }
    }
    dedupe(cues)
}

/// `00:00:01.000 --> 00:00:04.200 align:start` → (1000, 4200). Hours are optional.
fn parse_cue_timing(line: &str) -> Option<(u64, u64)> {
    let (a, rest) = line.split_once("-->")?;
    let b = rest.split_whitespace().next()?;
    Some((parse_vtt_time(a.trim())?, parse_vtt_time(b.trim())?))
}

fn parse_vtt_time(s: &str) -> Option<u64> {
    let (hms, ms) = s.split_once(['.', ','])?;
    let parts: Vec<u64> = hms.split(':').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let secs = match parts.as_slice() {
        [h, m, s] => (h * 60 + m) * 60 + s,
        [m, s] => m * 60 + s,
        _ => return None,
    };
    Some(secs * 1000 + ms.get(..3.min(ms.len()))?.parse::<u64>().ok()?)
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// Decodes the common entities and collapses whitespace.
fn clean_text(s: &str) -> String {
    let s = s
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace('\u{200b}', "");
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Drops cues that repeat text just shown: YouTube's rolling captions show each line twice
/// (first growing word by word, then as the upper line of the next cue).
fn dedupe(cues: Vec<CaptionCue>) -> Vec<CaptionCue> {
    let mut out: Vec<CaptionCue> = Vec::with_capacity(cues.len());
    for c in cues {
        let recent = out.iter().rev().take(3);
        if recent.clone().any(|p| p.text == c.text) {
            continue;
        }
        // A cue that extends the last one (word-by-word growth) replaces it.
        if let Some(last) = out.last_mut()
            && c.text.starts_with(&last.text)
            && c.start_ms.saturating_sub(last.start_ms) < 10_000
        {
            last.text = c.text;
            last.dur_ms = (c.start_ms + c.dur_ms).saturating_sub(last.start_ms);
            continue;
        }
        out.push(c);
    }
    out
}

/// Joins short cues into lines of roughly a sentence or two (15–40 s), so a transcript of a long
/// video stays readable. A pause of more than 3 s always starts a new line.
pub fn merge_cues(cues: &[CaptionCue]) -> Vec<CaptionCue> {
    const SOFT_MS: u64 = 15_000;
    const HARD_MS: u64 = 40_000;
    const MAX_CHARS: usize = 500;
    let mut out: Vec<CaptionCue> = Vec::new();
    let mut cur: Option<CaptionCue> = None;
    for c in cues {
        if let Some(g) = cur.as_mut() {
            let end = g.start_ms + g.dur_ms;
            let span = c.start_ms.saturating_sub(g.start_ms);
            let gap = c.start_ms.saturating_sub(end);
            let sentence_end = g.text.ends_with(['.', '!', '?', '。', '！', '？', '…']);
            let split = gap > 3_000 || span >= HARD_MS || g.text.chars().count() > MAX_CHARS || (span >= SOFT_MS && sentence_end);
            if !split {
                if !joins_without_space(&g.text, &c.text) {
                    g.text.push(' ');
                }
                g.text.push_str(&c.text);
                g.dur_ms = (c.start_ms + c.dur_ms).max(end) - g.start_ms;
                continue;
            }
            out.push(cur.take().unwrap());
        }
        cur = Some(c.clone());
    }
    out.extend(cur);
    out
}

fn joins_without_space(a: &str, b: &str) -> bool {
    let cjk = |c: char| matches!(c as u32, 0x3000..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF);
    a.chars().last().is_some_and(cjk) && b.chars().next().is_some_and(cjk)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json3_events_with_segments() {
        let j = r#"{"wireMagic":"pb3","events":[
            {"tStartMs":0,"dDurationMs":1000,"id":1},
            {"tStartMs":120,"dDurationMs":4000,"segs":[{"utf8":"All"},{"utf8":" right,","tOffsetMs":300},{"utf8":" so here","tOffsetMs":600}]},
            {"tStartMs":2000,"dDurationMs":10,"aAppend":1,"segs":[{"utf8":"\n"}]},
            {"tStartMs":4120,"dDurationMs":3000,"segs":[{"utf8":"we are &amp; elephants"}]}]}"#;
        let c = parse_json3(j).unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(c[0], CaptionCue { start_ms: 120, dur_ms: 4000, text: "All right, so here".into() });
        assert_eq!(c[1].text, "we are & elephants");
        assert!(parse_json3("not json").is_none());
    }

    #[test]
    fn vtt_rolling_auto_captions_are_deduplicated() {
        let v = "WEBVTT\nKind: captions\nLanguage: en\n\n\
            00:00:00.000 --> 00:00:02.330 align:start position:0%\n \nall<00:00:00.539><c> right</c><00:00:00.840><c> so</c>\n\n\
            00:00:02.330 --> 00:00:02.340 align:start position:0%\nall right so\n \n\n\
            00:00:02.340 --> 00:00:05.000 align:start position:0%\nall right so\nhere<00:00:03.000><c> we</c><00:00:03.500><c> are</c>\n\n\
            00:00:05.000 --> 00:00:05.010 align:start position:0%\nhere we are\n \n\n\
            00:00:05.010 --> 00:00:08.000 align:start position:0%\nhere we are\nin front of the elephants\n";
        let c = parse_vtt(v);
        let texts: Vec<&str> = c.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(texts, vec!["all right so", "here we are", "in front of the elephants"]);
        assert_eq!(c[0].start_ms, 0);
        assert_eq!(c[1].start_ms, 2340);
        assert_eq!(c[2].start_ms, 5010);
    }

    #[test]
    fn vtt_manual_captions_and_short_timestamps() {
        let v = "WEBVTT\n\n1\n00:01.500 --> 00:03.000\n<v Speaker>Hello &amp; welcome</v>\n\n2\n01:00:03.000 --> 01:00:04.250\nSecond line\nwraps here\n";
        let c = parse_vtt(v);
        assert_eq!(c.len(), 3);
        assert_eq!(c[0], CaptionCue { start_ms: 1500, dur_ms: 1500, text: "Hello & welcome".into() });
        assert_eq!(c[1].start_ms, 3_603_000);
        assert_eq!(c[2].text, "wraps here");
    }

    #[test]
    fn merging_respects_sentences_pauses_and_length() {
        let cue = |s: u64, t: &str| CaptionCue { start_ms: s, dur_ms: 2000, text: t.into() };
        let words = ["one", "two.", "three", "four", "five", "six", "seven", "eight.", "nine", "ten"];
        let mut cues: Vec<CaptionCue> = words.iter().enumerate().map(|(i, w)| cue(i as u64 * 2000, w)).collect();
        cues.push(cue(30_000, "later"));
        let m = merge_cues(&cues);
        let texts: Vec<&str> = m.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(texts, vec!["one two. three four five six seven eight.", "nine ten", "later"]);
        assert_eq!(m[0].dur_ms, 16_000);
        assert_eq!(m[1].start_ms, 16_000);
        let cjk = merge_cues(&[cue(0, "你好"), cue(2000, "世界")]);
        assert_eq!(cjk[0].text, "你好世界");
    }
}
