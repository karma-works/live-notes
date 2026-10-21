//! Per-language post-processing (SPEC §5 F2 step 7).

use ferrous_opencc::OpenCC;
use ferrous_opencc::config::BuiltinConfig;
use ln_types::{ChineseScript, Lang};

pub struct PostProcessor {
    to_simplified: Option<OpenCC>,
    to_traditional: Option<OpenCC>,
    script: ChineseScript,
}

impl PostProcessor {
    pub fn new(script: ChineseScript) -> Self {
        PostProcessor {
            to_simplified: OpenCC::from_config(BuiltinConfig::T2s).ok(),
            to_traditional: OpenCC::from_config(BuiltinConfig::S2t).ok(),
            script,
        }
    }

    pub fn set_script(&mut self, script: ChineseScript) {
        self.script = script;
    }

    /// Returns `None` when the text should be dropped (empty or a known hallucination).
    pub fn process(&self, text: &str, lang: &Lang) -> Option<String> {
        let mut t = collapse_repeats(text.trim());
        if lang.as_str() == "zh" {
            let conv = match self.script {
                ChineseScript::Simplified => self.to_simplified.as_ref(),
                ChineseScript::Traditional => self.to_traditional.as_ref(),
            };
            if let Some(c) = conv {
                t = c.convert(&t);
            }
            t = cjk_punctuation(&t);
        }
        let t = t.trim().to_string();
        if t.is_empty() || is_hallucination(&t) { None } else { Some(t) }
    }
}

/// Typical Whisper outputs on silence or noise.
pub fn is_hallucination(t: &str) -> bool {
    let lower = t.to_lowercase();
    const PHRASES: &[&str] = &[
        "untertitel von",
        "untertitel im auftrag",
        "untertitelung des zdf",
        "vielen dank fürs zuschauen",
        "thanks for watching",
        "thank you for watching",
        "subtitles by",
        "sous-titres réalisés",
        "sous-titrage",
        "merci d'avoir regardé",
        "subtítulos realizados por",
        "gracias por ver",
        "字幕由",
        "请不吝点赞",
        "明镜与点点",
        "谢谢观看",
    ];
    if PHRASES.iter().any(|p| lower.contains(p)) {
        return true;
    }
    let stripped: String = t.chars().filter(|c| c.is_alphanumeric()).collect();
    stripped.is_empty() || matches!(lower.as_str(), "you" | "." | "..." | "♪" | "[musik]" | "[music]")
}

/// Collapses loops: a unit repeated three or more times in a row is kept once. Works on words
/// ("okay okay okay okay" → "okay") and, for scripts without spaces, on characters
/// ("你好你好你好你好" → "你好", "我会去工作我会去工作…" → "我会去工作").
fn collapse_repeats(t: &str) -> String {
    let words: Vec<&str> = t.split_whitespace().collect();
    let words = collapse_units(&words, 6);
    let joined = words.join(" ");
    let chars: Vec<char> = joined.chars().collect();
    collapse_units(&chars, 12).into_iter().collect()
}

fn collapse_units<T: PartialEq + Copy>(items: &[T], max_unit: usize) -> Vec<T> {
    let mut out: Vec<T> = Vec::with_capacity(items.len());
    let mut i = 0;
    'outer: while i < items.len() {
        for n in 1..=max_unit {
            if i + 3 * n > items.len() {
                break;
            }
            let unit = &items[i..i + n];
            let mut reps = 1;
            while i + (reps + 1) * n <= items.len() && &items[i + reps * n..i + (reps + 1) * n] == unit {
                reps += 1;
            }
            if reps >= 3 {
                out.extend_from_slice(unit);
                i += reps * n;
                continue 'outer;
            }
        }
        out.push(items[i]);
        i += 1;
    }
    out
}

/// False when the text is far longer than anyone can speak in `duration_ms`: Whisper invented it.
pub fn plausible(text: &str, duration_ms: u64) -> bool {
    let secs = duration_ms as f32 / 1000.0;
    let cjk = text.chars().filter(|c| matches!(*c as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF)).count();
    let other = text.chars().filter(|c| c.is_alphanumeric()).count() - cjk;
    // Fast speech: ~8 CJK characters or ~20 letters per second. Generous margins.
    let load = cjk as f32 / 12.0 + other as f32 / 30.0;
    load <= secs + 2.0
}

/// Half-width punctuation between CJK characters becomes full-width.
fn cjk_punctuation(t: &str) -> String {
    let chars: Vec<char> = t.chars().collect();
    let is_cjk = |c: char| matches!(c as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF);
    let mut out = String::with_capacity(t.len());
    for (i, &c) in chars.iter().enumerate() {
        let prev_cjk = i > 0 && is_cjk(chars[i - 1]);
        let mapped = match c {
            ',' if prev_cjk => '，',
            '.' if prev_cjk => '。',
            '?' if prev_cjk => '？',
            '!' if prev_cjk => '！',
            ':' if prev_cjk => '：',
            ';' if prev_cjk => '；',
            other => other,
        };
        // Drop the space after converted punctuation.
        if c == ' ' && i > 0 && matches!(out.chars().last(), Some('，' | '。' | '？' | '！' | '：' | '；')) {
            continue;
        }
        out.push(mapped);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_script_and_punctuation() {
        let p = PostProcessor::new(ChineseScript::Simplified);
        assert_eq!(p.process("我們下週再討論預算.", &Lang::new("zh")).unwrap(), "我们下周再讨论预算。");
        let t = PostProcessor::new(ChineseScript::Traditional);
        assert_eq!(t.process("我们下周再讨论预算?", &Lang::new("zh")).unwrap(), "我們下週再討論預算？");
        assert_eq!(p.process("好, 我知道了", &Lang::new("zh")).unwrap(), "好，我知道了");
    }

    #[test]
    fn filters_hallucinations_and_repeats() {
        let p = PostProcessor::new(ChineseScript::Simplified);
        assert!(p.process("Untertitel von Stephanie Geiges", &Lang::new("de")).is_none());
        assert!(p.process("  ...  ", &Lang::new("de")).is_none());
        assert_eq!(p.process("Ja gut okay okay okay okay okay", &Lang::new("de")).unwrap(), "Ja gut okay");
        assert_eq!(p.process("Das Budget steht.", &Lang::new("de")).unwrap(), "Das Budget steht.");
        assert_eq!(p.process("Ja ja, das passt.", &Lang::new("de")).unwrap(), "Ja ja, das passt.");
    }

    #[test]
    fn chinese_loops_from_a_real_recording() {
        let p = PostProcessor::new(ChineseScript::Simplified);
        let zh = Lang::new("zh");
        assert_eq!(p.process(&"你好".repeat(29), &zh).unwrap(), "你好");
        let loop_ = "我会去工作我会去工作我会去工作我会去工作我会去工作工作我会去工作工作工作工作";
        let out = p.process(loop_, &zh).unwrap();
        assert!(out.chars().count() <= 12, "{out}");
        assert!(out.starts_with("我会去工作"), "{out}");
        // Normal Chinese with a short doubled word stays as it is.
        assert_eq!(p.process("我们谢谢你们，看看再说。", &zh).unwrap(), "我们谢谢你们，看看再说。");
    }

    #[test]
    fn implausibly_long_text_for_the_duration() {
        let english = "we have a subject. Again, you don't have to. That I should. Let's say that there's a few units that are there is even a unit apart from two in the front of the other can have access to you. It can hang out straight, but it doesn't seem like that there are no more from there.";
        assert!(!plausible(english, 2000));
        assert!(plausible(english, 15000));
        assert!(plausible("你好，我今天去上班。", 2400));
        assert!(plausible("Können wir das Budget für das erste Quartal bis Freitag festlegen?", 4000));
    }
}

#[cfg(test)]
mod keep_words {
    #[test]
    fn ordinary_words_survive() {
        for s in ["Können wir das Budget festlegen?", "Pouvez-vous m'envoyer le contrat ?", "Mississippi", "看看再说"] {
            assert_eq!(super::collapse_repeats(s), s);
        }
    }
}
