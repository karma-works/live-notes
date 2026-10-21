//! Structure-aware chunking: 256–512 tokens, 15 % overlap (SPEC §8.3).

use crate::Block;
use ln_index::NewChunk;

const TARGET: usize = 400;
const MAX: usize = 512;
const OVERLAP: usize = 60;

/// Rough token count: one token per CJK character, ~1.3 per word otherwise.
pub fn estimate_tokens(text: &str) -> usize {
    let cjk = text.chars().filter(|c| matches!(*c as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF)).count();
    let words = text.split_whitespace().filter(|w| w.chars().any(|c| c.is_alphanumeric() && !(c as u32 >= 0x3040))).count();
    cjk + words * 13 / 10
}

/// Merges small blocks with the same locator kind and splits large ones.
pub fn chunk_blocks(blocks: &[crate::Block], file_lang: Option<&str>) -> Vec<NewChunk> {
    let mut out: Vec<NewChunk> = Vec::new();
    let mut cur: Option<(String, Block)> = None;
    for b in blocks {
        for piece in split_large(&b.text) {
            let piece_tokens = estimate_tokens(&piece);
            if let Some((text, first)) = cur.as_mut() {
                let same_place = first.locator.page == b.locator.page && first.locator.t_ms.is_some() == b.locator.t_ms.is_some();
                if same_place && estimate_tokens(text) + piece_tokens <= TARGET {
                    text.push_str("\n\n");
                    text.push_str(&piece);
                    continue;
                }
                let (t, f) = cur.take().unwrap();
                push(&mut out, t, &f, file_lang);
            }
            cur = Some((piece, b.clone()));
        }
    }
    if let Some((t, f)) = cur {
        push(&mut out, t, &f, file_lang);
    }
    out
}

fn push(out: &mut Vec<NewChunk>, text: String, first: &Block, file_lang: Option<&str>) {
    let lang = first.lang.clone().or_else(|| crate::detect_lang(&text)).or_else(|| file_lang.map(str::to_string));
    out.push(NewChunk { text, lang, locator: first.locator.clone() });
}

/// Splits text above MAX tokens at sentence boundaries, with ~15 % overlap.
fn split_large(text: &str) -> Vec<String> {
    if estimate_tokens(text) <= MAX {
        return vec![text.to_string()];
    }
    let sentences = sentences(text);
    let mut out = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut cur_tokens = 0;
    for s in sentences {
        let t = estimate_tokens(s).max(1);
        if cur_tokens + t > TARGET && !cur.is_empty() {
            out.push(cur.concat().trim().to_string());
            // Overlap: keep trailing sentences up to OVERLAP tokens.
            let mut keep = Vec::new();
            let mut kt = 0;
            for s in cur.iter().rev() {
                kt += estimate_tokens(s);
                if kt > OVERLAP {
                    break;
                }
                keep.push(*s);
            }
            keep.reverse();
            cur_tokens = keep.iter().map(|s| estimate_tokens(s)).sum();
            cur = keep;
        }
        cur.push(s);
        cur_tokens += t;
    }
    if !cur.is_empty() {
        out.push(cur.concat().trim().to_string());
    }
    // A single endless "sentence" (no punctuation) is cut hard.
    out.into_iter()
        .flat_map(|c| {
            if estimate_tokens(&c) <= MAX * 2 {
                vec![c]
            } else {
                let chars: Vec<char> = c.chars().collect();
                chars.chunks(1500).map(|w| w.iter().collect()).collect()
            }
        })
        .collect()
}

fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = text.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let end_cjk = matches!(c, '。' | '！' | '？' | '；');
        let end_latin = matches!(c, '.' | '!' | '?' | '\n') && it.peek().is_none_or(|(_, n)| n.is_whitespace());
        if end_cjk || end_latin {
            let e = i + c.len_utf8();
            out.push(&text[start..e]);
            start = e;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ln_index::Locator;

    fn b(t: &str, page: Option<u32>) -> Block {
        Block { text: t.into(), locator: Locator { page, ..Default::default() }, lang: None }
    }

    #[test]
    fn merges_small_and_splits_large() {
        let small = vec![b("Erster Absatz.", None), b("Zweiter Absatz.", None)];
        assert_eq!(chunk_blocks(&small, Some("de")).len(), 1);
        let sentence = "Das ist ein ziemlich langer Satz über das Budget im nächsten Quartal. ";
        let big = sentence.repeat(120);
        let chunks = chunk_blocks(&[b(&big, Some(1))], None);
        assert!(chunks.len() >= 3);
        assert!(chunks.iter().all(|c| estimate_tokens(&c.text) <= MAX));
        // Overlap: the start of chunk 2 repeats the end of chunk 1.
        let tail: String = chunks[0].text.chars().rev().take(30).collect::<Vec<_>>().into_iter().rev().collect();
        assert!(chunks[1].text.contains(tail.trim()));
        assert_eq!(chunks[0].lang.as_deref(), Some("de"));
    }

    #[test]
    fn pages_are_not_merged_and_cjk_counts() {
        let c = chunk_blocks(&[b("Seite eins", Some(1)), b("Seite zwei", Some(2))], None);
        assert_eq!(c.len(), 2);
        assert_eq!(estimate_tokens("我们下周再讨论"), 7);
        let zh = "我们下周再讨论预算。".repeat(100);
        let chunks = chunk_blocks(&[b(&zh, None)], None);
        assert!(chunks.len() >= 2);
    }
}
