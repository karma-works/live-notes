/// True if the text contains Han, Hiragana, Katakana or Hangul characters.
pub fn has_cjk(s: &str) -> bool {
    s.chars().any(is_cjk)
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F)
}

/// Runs of consecutive CJK characters in a query.
pub fn cjk_runs(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if is_cjk(c) {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Builds a safe FTS5 query from free text: each word quoted, OR-combined, last word as prefix.
/// Returns `None` when nothing searchable is left (e.g. only CJK or punctuation).
pub fn fts_query(text: &str) -> Option<String> {
    let words: Vec<String> = text
        .split(|c: char| !(c.is_alphanumeric() || c == '_') || is_cjk(c))
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect();
    if words.is_empty() {
        return None;
    }
    let n = words.len();
    let parts: Vec<String> = words
        .into_iter()
        .enumerate()
        .map(|(i, w)| if i == n - 1 && w.chars().count() >= 2 { format!("\"{w}\"*") } else { format!("\"{w}\"") })
        .collect();
    Some(parts.join(" OR "))
}

/// A window of `max_len` bytes around the first query term, with highlight ranges.
pub fn snippet(text: &str, query: &str, max_len: usize) -> (String, Vec<(usize, usize)>) {
    let lower = text.to_lowercase();
    // Lowercasing can change byte lengths for some scripts; fall back to no highlights then.
    let same_len = lower.len() == text.len();
    let mut terms: Vec<String> = query
        .split(|c: char| !(c.is_alphanumeric() || c == '_') || is_cjk(c))
        .filter(|w| w.chars().count() >= 2)
        .map(|w| w.to_lowercase())
        .collect();
    terms.extend(cjk_runs(query));
    let first = if same_len { terms.iter().filter_map(|t| lower.find(t.as_str())).min() } else { None };
    let start = first.map(|p| p.saturating_sub(max_len / 3)).unwrap_or(0);
    let start = floor_char(text, start);
    let end = floor_char(text, (start + max_len).min(text.len()));
    let mut snip = text[start..end].replace('\n', " ");
    let mut prefix = 0;
    if start > 0 {
        snip.insert_str(0, "… ");
        prefix = "… ".len();
    }
    if end < text.len() {
        snip.push_str(" …");
    }
    let mut hl = Vec::new();
    if same_len {
        let window = &lower[start..end];
        for t in &terms {
            let mut from = 0;
            while let Some(p) = window[from..].find(t.as_str()) {
                let s = from + p;
                hl.push((prefix + s, prefix + s + t.len()));
                from = s + t.len();
            }
        }
        hl.sort();
        hl.dedup();
    }
    (snip, hl)
}

fn floor_char(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_queries() {
        assert_eq!(fts_query("Budget nächstes Quartal").unwrap(), "\"budget\" OR \"nächstes\" OR \"quartal\"*");
        assert_eq!(fts_query("\"); DROP"), Some("\"drop\"*".into()));
        assert!(fts_query("下季度预算").is_none());
        assert_eq!(cjk_runs("Budget 下季度 预算"), vec!["下季度", "预算"]);
    }

    #[test]
    fn snippets_highlight_terms() {
        let t = "Lorem ipsum. Das Budget für Q1 wird nächste Woche besprochen.";
        let (s, h) = snippet(t, "budget", 40);
        assert!(s.contains("Budget"));
        let (a, b) = h[0];
        assert_eq!(&s[a..b].to_lowercase(), "budget");
    }
}
