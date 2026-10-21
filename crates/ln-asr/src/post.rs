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

/// "the the the the" → "the"; repeated phrases of up to 6 words at the end are collapsed.
fn collapse_repeats(t: &str) -> String {
    let words: Vec<&str> = t.split_whitespace().collect();
    if words.len() < 6 {
        return t.to_string();
    }
    for n in 1..=6 {
        let mut reps = 1;
        let mut i = words.len();
        while i >= 2 * n && words[i - n..i] == words[i - 2 * n..i - n] {
            reps += 1;
            i -= n;
        }
        if reps >= 3 {
            let keep = &words[..i];
            return keep.join(" ");
        }
    }
    t.to_string()
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
    }
}
