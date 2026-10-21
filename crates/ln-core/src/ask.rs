//! Search (SPEC §5 F5) and questions with sources (§5 F6).

use crate::events::tr;
use crate::{Core, CoreEvent};
use ln_ai::ChatRequest;
use ln_ai::prompts::{self, ContextChunk};
use ln_index::{Locator, SearchFilter, SearchHit};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, PartialEq)]
pub enum AskScope {
    All,
    Note(PathBuf),
    Folder(PathBuf),
    LastDays(u32),
    /// "Ask the running conversation".
    CurrentRecording,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Citation {
    pub n: usize,
    pub path: PathBuf,
    pub title: String,
    pub locator: Locator,
    pub snippet: String,
}

/// Below this cosine similarity a purely semantic hit is dropped from search results.
const WEAK_SIMILARITY: f32 = 0.35;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static CANCELLED: AtomicU64 = AtomicU64::new(0);

impl Core {
    /// Hybrid search. Blocking; call it off the UI thread. Falls back to full-text search when
    /// the embedding model is not available.
    pub fn search(&self, query: &str, filter: &SearchFilter, limit: usize) -> Vec<SearchHit> {
        if query.trim().is_empty() {
            return vec![];
        }
        let emb = if self.embedder_available() && self.embedder_warm() {
            self.embed(&[query.to_string()], true).ok().and_then(|mut v| v.pop())
        } else {
            None
        };
        let hits = self.index.lock().search(query, emb.as_deref(), filter, limit).unwrap_or_default();
        // A hit found only by meaning, and only weakly, is noise in a result list.
        hits.into_iter().filter(|h| !(h.highlights.is_empty() && h.similarity.is_some_and(|s| s < WEAK_SIMILARITY))).collect()
    }

    /// Search results grouped by file (what the search panel shows).
    pub fn search_files(&self, query: &str, filter: &SearchFilter, limit: usize) -> Vec<SearchHit> {
        ln_index::group_by_file(self.search(query, filter, limit * 3)).into_iter().take(limit).collect()
    }

    pub(crate) fn filter_for(&self, scope: &AskScope) -> SearchFilter {
        let mut f = SearchFilter::default();
        match scope {
            AskScope::Note(p) | AskScope::Folder(p) => f.path_prefix = Some(p.clone()),
            AskScope::LastDays(d) => f.since = Some(chrono::Utc::now().timestamp() - *d as i64 * 86_400),
            AskScope::All | AskScope::CurrentRecording => {}
        }
        f
    }

    /// Source ids whose content must never go to a cloud model.
    pub(crate) fn local_only_sources(&self) -> Vec<i64> {
        let s = self.settings();
        let ix = self.index.lock();
        s.sources.iter().filter(|x| x.local_only).filter_map(|x| ix.source_id(&x.path).ok().flatten()).collect()
    }

    /// Asks a question; the answer streams as `CoreEvent::Answer`. Returns the answer id.
    pub fn ask(self: &Arc<Self>, question: &str, scope: AskScope, history: Vec<(String, String)>) -> u64 {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let me = self.clone();
        let q = question.to_string();
        std::thread::spawn(move || me.answer(id, &q, &scope, &history));
        id
    }

    pub fn cancel_answer(&self, id: u64) {
        CANCELLED.store(id, Ordering::Relaxed);
    }

    fn answer(self: &Arc<Self>, id: u64, question: &str, scope: &AskScope, history: &[(String, String)]) {
        let done = |text: String, citations: Vec<Citation>| self.emit(CoreEvent::Answer { id, text, done: true, citations });
        let role = match self.assistant() {
            Ok(r) => r,
            Err(u) => return done(self.unavailable_text(&u), vec![]),
        };
        let mut hits = self.search(question, &self.filter_for(scope), 12);
        if role.cloud {
            let local = self.local_only_sources();
            hits.retain(|h| !local.contains(&h.source_id));
        }
        hits.truncate(8);
        let transcript = if *scope == AskScope::CurrentRecording { self.current_transcript_text(6000) } else { None };
        let mut ctx: Vec<ContextChunk<'_>> = Vec::new();
        let rec_title = tr(self.ui_language(), "Laufendes Gespräch", "Current conversation");
        if let Some(t) = &transcript {
            ctx.push(ContextChunk { title: &rec_title, text: t });
        }
        ctx.extend(hits.iter().map(|h| ContextChunk { title: &h.title, text: &h.text }));
        if ctx.is_empty() {
            return done(
                tr(
                    self.ui_language(),
                    "Dazu habe ich in deinen Notizen und Dateien nichts gefunden.",
                    "Nothing about this in your notes and files.",
                ),
                vec![],
            );
        }
        let offset = usize::from(transcript.is_some());
        let citations: Vec<Citation> = hits
            .iter()
            .enumerate()
            .map(|(i, h)| Citation {
                n: i + 1 + offset,
                path: h.path.clone(),
                title: h.title.clone(),
                locator: h.locator.clone(),
                snippet: h.snippet.clone(),
            })
            .collect();
        let req = ChatRequest::new(role.model.clone(), prompts::answer(question, &ctx, history));
        let c = role.client.clone();
        let me = self.clone();
        let cites = citations.clone();
        let mut acc = String::new();
        let mut last = std::time::Instant::now();
        let result = self.runtime().block_on(async {
            c.chat_stream(&req, |d| {
                acc.push_str(d);
                if last.elapsed().as_millis() > 60 {
                    last = std::time::Instant::now();
                    me.emit(CoreEvent::Answer { id, text: acc.clone(), done: false, citations: cites.clone() });
                }
                CANCELLED.load(Ordering::Relaxed) != id
            })
            .await
        });
        match result {
            Ok(r) => {
                self.costs.record_chat(&role.client.provider().id, &role.model, "answer", None, &r.usage);
                done(r.text, citations)
            }
            Err(ln_ai::Error::Cancelled) => done(acc, citations),
            Err(e) => done(e.user_message(self.ui_language()), vec![]),
        }
    }
}
