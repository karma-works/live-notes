use ln_types::Lang;

/// Below this confidence (and for short sections) the previous language is kept (SPEC §5 F2 step 5).
pub const LANG_CONFIDENCE_MIN: f32 = 0.6;
const SHORT_MS: u64 = 2_000;

/// Picks the language among the user's languages. `probs` are raw detector probabilities for
/// any languages; they are renormalised over `allowed`. Returns (language, confidence).
pub fn choose_language(probs: &[(Lang, f32)], allowed: &[Lang], previous: Option<&Lang>, duration_ms: u64) -> (Lang, f32) {
    let fallback = || previous.cloned().or_else(|| allowed.first().cloned()).unwrap_or_else(|| Lang::new("en"));
    let mut cand: Vec<(Lang, f32)> = if allowed.is_empty() {
        probs.to_vec()
    } else {
        probs.iter().filter(|(l, _)| allowed.contains(l)).cloned().collect()
    };
    let total: f32 = cand.iter().map(|(_, p)| *p).sum();
    if total <= f32::EPSILON {
        return (fallback(), 0.0);
    }
    cand.iter_mut().for_each(|(_, p)| *p /= total);
    cand.sort_by(|a, b| b.1.total_cmp(&a.1));
    let (best, conf) = cand[0].clone();
    if conf < LANG_CONFIDENCE_MIN
        && duration_ms < SHORT_MS
        && let Some(prev) = previous
    {
        return (prev.clone(), conf);
    }
    (best, conf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(s: &str) -> Lang {
        Lang::new(s)
    }

    #[test]
    fn restricts_to_my_languages() {
        // Detector thinks Dutch, but Dutch is not one of mine: German wins.
        let probs = vec![(l("nl"), 0.5), (l("de"), 0.4), (l("en"), 0.1)];
        let mine = Lang::main_languages();
        let (lang, conf) = choose_language(&probs, &mine, None, 5_000);
        assert_eq!(lang, l("de"));
        assert!((conf - 0.8).abs() < 1e-6);
    }

    #[test]
    fn keeps_previous_for_short_unsure_sections() {
        let probs = vec![(l("de"), 0.5), (l("en"), 0.5)];
        let mine = Lang::main_languages();
        assert_eq!(choose_language(&probs, &mine, Some(&l("zh")), 1_000).0, l("zh"));
        assert_eq!(choose_language(&probs, &mine, Some(&l("zh")), 4_000).0, l("de"));
        assert_eq!(choose_language(&[], &mine, None, 1000).0, l("de"));
    }
}
