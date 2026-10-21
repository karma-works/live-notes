//! Live suggestions during a conversation (SPEC §5 F8).

use crate::recording::State;
use crate::{Core, CoreEvent};
use ln_ai::ChatRequest;
use ln_ai::prompts::{self, ContextChunk};
use ln_index::{SearchFilter, SearchHit};
use ln_types::{Channel, LiveSuggestions, Segment};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq)]
pub struct SuggestionCard {
    pub id: u64,
    /// What was searched.
    pub query: String,
    /// The question heard in the conversation, if it was one.
    pub question: Option<String>,
    pub hits: Vec<SearchHit>,
    pub at_ms: u64,
}

/// Why a lookup starts.
pub struct Trigger {
    recent: String,
    question: Option<String>,
    fallback_query: String,
    at_ms: u64,
}

#[derive(Default)]
pub struct LiveSuggester {
    recent: VecDeque<Segment>,
    last_card: Option<Instant>,
    last_trigger_ms: u64,
    pub cards: Vec<SuggestionCard>,
    dismissed: Vec<String>,
    busy: Arc<AtomicBool>,
}

static NEXT: AtomicU64 = AtomicU64::new(1);

impl LiveSuggester {
    /// Called for every transcribed segment; returns a trigger when a lookup should run.
    pub fn observe(&mut self, seg: &Segment, cfg: &LiveSuggestions) -> Option<Trigger> {
        self.recent.push_back(seg.clone());
        while self.recent.front().is_some_and(|s| seg.end_ms.saturating_sub(s.end_ms) > 90_000) {
            self.recent.pop_front();
        }
        if !cfg.enabled || self.busy.load(Ordering::Relaxed) {
            return None;
        }
        if self.last_card.is_some_and(|t| t.elapsed().as_secs() < cfg.frequency.min_gap_secs()) {
            return None;
        }
        let question = seg.channel == Channel::Others && is_question(&seg.text);
        let topic = seg.end_ms.saturating_sub(self.last_trigger_ms) >= cfg.frequency.topic_window_secs() * 1000
            && self.recent.iter().map(|s| s.text.chars().count()).sum::<usize>() > 80;
        if !question && !topic {
            return None;
        }
        self.last_trigger_ms = seg.end_ms;
        let recent = self.recent.iter().map(|s| format!("{}: {}", s.channel.id(), s.text)).collect::<Vec<_>>().join("\n");
        Some(Trigger { recent, question: question.then(|| seg.text.clone()), fallback_query: seg.text.clone(), at_ms: seg.end_ms })
    }

    fn similar_to_dismissed(&self, q: &str) -> bool {
        let words = |s: &str| -> std::collections::HashSet<String> { s.to_lowercase().split_whitespace().map(str::to_string).collect() };
        let a = words(q);
        self.dismissed.iter().any(|d| {
            let b = words(d);
            let inter = a.intersection(&b).count();
            inter * 10 >= a.len().max(1) * 6
        })
    }

    pub fn pin(&mut self, id: u64) -> Option<String> {
        let c = self.cards.iter().find(|c| c.id == id)?;
        let h = c.hits.first()?;
        Some(format!("{} — [{}]({})", c.question.as_deref().unwrap_or(&c.query), h.title, h.path.display()))
    }

    pub fn dismiss(&mut self, id: u64) {
        if let Some(i) = self.cards.iter().position(|c| c.id == id) {
            let c = self.cards.remove(i);
            self.dismissed.push(c.query);
        }
    }
}

pub fn is_question(text: &str) -> bool {
    let t = text.trim();
    if t.ends_with('?') || t.ends_with('？') || t.ends_with('吗') || t.ends_with("吗。") || t.ends_with('呢') || t.starts_with('¿') {
        return true;
    }
    let first = t.split_whitespace().next().unwrap_or("").to_lowercase();
    let first = first.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-');
    const W: &[&str] = &[
        "wer", "wie", "was", "wann", "wo", "warum", "wieso", "weshalb", "welche", "welcher", "welches", "kannst", "können", "könnten", "hast", "habt", "gibt",
        "who", "what", "when", "where", "why", "how", "which", "can", "could", "do", "does", "did", "will", "would", "should",
        "qui", "que", "quoi", "quand", "où", "pourquoi", "comment", "est-ce", "quel", "quelle",
        "qué", "quién", "cuándo", "dónde", "cómo", "cuál",
    ];
    W.contains(&first)
}

pub fn spawn(core: &Arc<Core>, t: Trigger, note_path: PathBuf, note_id: String, state: Arc<Mutex<State>>) {
    let busy = state.lock().suggester.busy.clone();
    busy.store(true, Ordering::Relaxed);
    let me = core.clone();
    std::thread::spawn(move || {
        run(&me, t, note_path, &note_id, &state);
        busy.store(false, Ordering::Relaxed);
    });
}

fn run(core: &Arc<Core>, t: Trigger, note_path: PathBuf, note_id: &str, state: &Arc<Mutex<State>>) {
    let cfg = core.settings().live_suggestions;
    let role = core.assistant().ok();
    // Step 2: ask the model what to look up; without a model use the last section directly.
    let query = match &role {
        Some(r) => match core.chat_once(r, prompts::suggestion_query(&t.recent), true, "suggestion", Some(note_id)).ok().and_then(|x| prompts::parse_suggestion_query(&x)) {
            Some(q) if q.relevant && !q.query.trim().is_empty() => q.query,
            Some(_) => return,
            None => t.fallback_query.clone(),
        },
        None => t.fallback_query.clone(),
    };
    if state.lock().suggester.similar_to_dismissed(&query) {
        return;
    }
    // Step 3: local search, excluding the running note.
    let filter = SearchFilter { exclude_paths: vec![note_path], ..Default::default() };
    let mut hits = core.search_files(&query, &filter, 3);
    if role.as_ref().is_some_and(|r| r.cloud) {
        let local = core.local_only_sources();
        hits.retain(|h| !local.contains(&h.source_id));
    }
    if hits.is_empty() {
        return;
    }
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let card = SuggestionCard { id, query: query.clone(), question: t.question.clone(), hits: hits.clone(), at_ms: t.at_ms };
    {
        let mut st = state.lock();
        st.suggester.cards.push(card.clone());
        st.suggester.last_card = Some(Instant::now());
    }
    core.emit(CoreEvent::Suggestion(card));
    // Step 4: short answer under the hits.
    let Some(role) = role.filter(|_| cfg.answers) else { return };
    let lang = core.recording_segments().last().map(|s| s.lang.to_string()).unwrap_or_else(|| core.ui_language().code().into());
    let ctx: Vec<ContextChunk<'_>> = hits.iter().map(|h| ContextChunk { title: &h.title, text: &h.text }).collect();
    let req = ChatRequest::new(role.model.clone(), prompts::live_answer(t.question.as_deref().unwrap_or(&query), &ctx, &lang));
    let c = role.client.clone();
    let mut acc = String::new();
    let me = core.clone();
    let r = core.runtime().block_on(async {
        c.chat_stream(&req, |d| {
            acc.push_str(d);
            me.emit(CoreEvent::SuggestionAnswer { id, text: acc.clone(), done: false });
            true
        })
        .await
    });
    if let Ok(resp) = &r {
        core.costs.record_chat(&role.client.provider().id, &role.model, "suggestion", Some(note_id), &resp.usage);
    }
    core.emit(CoreEvent::SuggestionAnswer { id, text: r.map(|x| x.text).unwrap_or(acc), done: true });
}

#[cfg(test)]
mod tests {
    use super::*;
    use ln_types::Lang;

    fn seg(ch: Channel, t: &str, end: u64) -> Segment {
        Segment { channel: ch, start_ms: end.saturating_sub(2000), end_ms: end, lang: Lang::new("de"), text: t.into(), lang_confidence: 1.0 }
    }

    #[test]
    fn questions_in_five_languages() {
        for q in ["Wann kommt das Angebot?", "Kannst du mir die Zahlen schicken", "What is the budget", "我们什么时候开会吗", "¿Cuándo llega?", "Pourquoi pas"] {
            assert!(is_question(q), "{q}");
        }
        assert!(!is_question("Das Budget steht."));
    }

    #[test]
    fn triggers_respect_gap_and_channel() {
        let cfg = LiveSuggestions::default();
        let mut s = LiveSuggester::default();
        assert!(s.observe(&seg(Channel::Me, "Wann kommt das?", 5_000), &cfg).is_none(), "own questions do not trigger");
        let t = s.observe(&seg(Channel::Others, "Wann kommt das Angebot?", 8_000), &cfg);
        assert!(t.unwrap().question.is_some());
        s.last_card = Some(Instant::now());
        assert!(s.observe(&seg(Channel::Others, "Und der Preis?", 9_000), &cfg).is_none(), "min gap");
        s.dismissed.push("Angebot Li Preis".into());
        assert!(s.similar_to_dismissed("Preis Angebot Li"));
        assert!(!s.similar_to_dismissed("Urlaub im Sommer"));
    }
}
