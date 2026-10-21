//! Note categories: the user names them and gives each a macOS tag colour; a decision model
//! (Jev on OpenRouter, or OpenAI's Decisions API) sorts each note into them with one request
//! that holds one yes/no question per category. The result lands in the note's front matter
//! (`categories:`), so the Markdown file stays the source of truth.

use crate::ai::Unavailable;
use crate::{Core, CoreEvent};
use ln_ai::{Client, YesNo};
use ln_notes::{Note, NoteFile, SectionKind};
use ln_types::{Category, DecisionProvider};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

/// Text above ~32 KB puts Jev into its slower long-context mode; notes are cut before that.
const MAX_STATE_BYTES: usize = 30_000;

impl Core {
    /// The decision model to use now. `Auto` prefers OpenRouter when both keys are set.
    pub(crate) fn decisions_client(&self) -> Result<(Client, String), Unavailable> {
        let s = self.settings();
        let c = &s.categories;
        let id = match c.provider {
            DecisionProvider::OpenRouter => "openrouter",
            DecisionProvider::OpenAi => "openai",
            DecisionProvider::Auto if self.has_api_key("openrouter") => "openrouter",
            DecisionProvider::Auto if self.has_api_key("openai") => "openai",
            DecisionProvider::Auto => return Err(Unavailable::NoKey("OpenRouter / OpenAI".into())),
        };
        let p = s.provider(id).cloned().ok_or(Unavailable::ModelMissing)?;
        if s.privacy.offline_mode {
            return Err(Unavailable::Offline);
        }
        if !self.has_api_key(id) {
            return Err(Unavailable::NoKey(p.name.clone()));
        }
        if self.cost_limit_reached() {
            return Err(Unavailable::CostLimit);
        }
        let model = if id == "openrouter" { c.openrouter_model.clone() } else { c.openai_model.clone() };
        let mut client = Client::new(p, self.api_key(id));
        client.deny_data_collection = s.privacy.openrouter_deny_data_collection;
        Ok((client, model))
    }

    /// "OpenRouter · typesafe/jev-1.14" when notes can be sorted now, else why not.
    pub fn decisions_status(&self) -> Result<String, String> {
        self.decisions_client().map(|(c, m)| format!("{} · {m}", c.provider().name)).map_err(|u| self.unavailable_text(&u))
    }

    /// Sorts a note into the categories in the background, if automatic sorting is on.
    pub fn classify_note(self: &Arc<Self>, path: &Path) {
        let s = self.settings();
        if !s.categories.auto_assign || s.categories.list.is_empty() {
            return;
        }
        let me = self.clone();
        let path = path.to_path_buf();
        std::thread::spawn(move || {
            if let Err(e) = me.classify_note_now(&path, false) {
                tracing::warn!("categories for {}: {e}", path.display());
            }
        });
    }

    /// Sorts every note that has no category yet (after the user added categories).
    /// Reports how many notes got one.
    pub fn classify_all_notes(self: &Arc<Self>) {
        let me = self.clone();
        std::thread::spawn(move || {
            let ui = me.ui_language();
            let mut sorted = 0;
            for n in me.list_notes().into_iter().filter(|n| n.categories.is_empty()) {
                match me.classify_note_now(&n.path, false) {
                    Ok(Some(c)) if !c.is_empty() => sorted += 1,
                    Ok(_) => {}
                    Err(e) => {
                        me.emit(CoreEvent::Notice(e));
                        return;
                    }
                }
            }
            me.emit(CoreEvent::Notice(crate::events::tr(
                ui,
                &format!("{sorted} Notizen wurden Kategorien zugeordnet."),
                &format!("{sorted} notes were sorted into categories."),
            )));
        });
    }

    /// One decision request for the note; writes the categories into its front matter.
    /// Returns the categories, or `None` when there was nothing to decide (empty note, no
    /// categories, content unchanged since the last run unless `force`).
    pub fn classify_note_now(&self, path: &Path, force: bool) -> Result<Option<Vec<String>>, String> {
        let s = self.settings();
        let list = s.categories.list.clone();
        if list.is_empty() {
            return Ok(None);
        }
        let note = NoteFile::open(path).and_then(|f| f.parse()).map_err(|e| e.to_string())?;
        let state = note_state(&note);
        if state.chars().filter(|c| c.is_alphanumeric()).count() < 3 {
            return Ok(None);
        }
        let hash = state_hash(&state, &list);
        if !force && self.classified.lock().get(path) == Some(&hash) {
            return Ok(None);
        }
        let (client, model) = self.decisions_client().map_err(|u| self.unavailable_text(&u))?;
        let questions = questions(&list);
        let d = self
            .runtime()
            .block_on(async { client.decide_yes_no(&model, &state, &questions).await })
            .map_err(|e| e.user_message(self.ui_language()))?;
        let note_id = note.front.as_ref().and_then(|f| f.id.clone());
        self.costs.record_chat(&client.provider().id, &model, "categories", note_id.as_deref(), &d.usage);
        let threshold = s.categories.threshold;
        let mut chosen: Vec<String> = list.iter().zip(&d.yes).filter(|(_, (_, p))| *p >= threshold).map(|(c, _)| c.name.clone()).collect();
        tracing::info!("categories for {}: {:?} → {:?}", path.display(), d.yes, chosen);
        // Re-read right before writing: the user may have edited meanwhile. Categories the
        // user typed by hand that are not in the list stay.
        let mut nf = NoteFile::open(path).map_err(|e| e.to_string())?;
        let mut current = nf.parse().map_err(|e| e.to_string())?;
        chosen.extend(current.categories().into_iter().filter(|c| !list.iter().any(|x| &x.name == c)));
        self.classified.lock().insert(path.to_path_buf(), hash);
        if current.categories() != chosen {
            current.set_categories(chosen.clone());
            nf.text = current.render();
            nf.save(true).map_err(|e| e.to_string())?;
            self.emit(CoreEvent::NoteFileChanged(path.to_path_buf()));
            self.emit(CoreEvent::NotesChanged);
        }
        Ok(Some(chosen))
    }
}

/// One yes/no question per category, keyed `c0`, `c1`, … in list order.
pub(crate) fn questions(list: &[Category]) -> Vec<YesNo> {
    list.iter()
        .enumerate()
        .map(|(i, c)| {
            let name = c.name.trim();
            let hint = c.hint.trim();
            let about = if hint.is_empty() { format!("\"{name}\"") } else { format!("\"{name}\" ({hint})") };
            YesNo {
                name: format!("c{i}"),
                instructions: format!(
                    "Does this personal note belong to the category {about}? The note can be in any language (German, English, Chinese, French, Spanish …)."
                ),
                yes: format!("The note is mainly or to a substantial part about {about}."),
                no: format!("The note is about something else, or mentions {about} only in passing."),
            }
        })
        .collect()
}

/// What the model reads: title, the user's text and sections, then the transcript without
/// timestamps; cut to [`MAX_STATE_BYTES`].
pub(crate) fn note_state(note: &Note) -> String {
    let mut s = String::new();
    if let Some(t) = note.title().filter(|t| !t.trim().is_empty()) {
        s.push_str(&format!("Title: {}\n\n", t.trim()));
    }
    s.push_str(note.preamble.trim());
    for sec in note.sections.iter().filter(|x| x.kind != SectionKind::Transcript) {
        if !sec.body.trim().is_empty() {
            s.push_str(&format!("\n\n{}\n{}", sec.heading, sec.body.trim()));
        }
    }
    let speakers = note.speakers();
    let lines = note.transcript_lines();
    if !lines.is_empty() {
        s.push_str("\n\nTranscript:\n");
        for l in lines {
            let who = l.channel(&speakers).map(|c| c.id().to_string()).unwrap_or_else(|| l.label.clone());
            s.push_str(&format!("{who}: {}\n", l.text));
        }
    }
    if s.len() > MAX_STATE_BYTES {
        let mut end = MAX_STATE_BYTES;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s.trim().to_string()
}

fn state_hash(state: &str, list: &[Category]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    state.hash(&mut h);
    for c in list {
        c.name.hash(&mut h);
        c.hint.hash(&mut h);
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ln_types::TagColor;

    #[test]
    fn state_skips_markers_and_timestamps() {
        let text = "---\nid: X\ntitle: Kurs\ncategories:\n- Alt\n---\n\n<!-- ln:notes -->\n## Meine Notizen\n- 声调 üben\n\n<!-- ln:transcript -->\n## Transkript\n[00:00:03] Ich (de): Wie spricht man das aus?\n";
        let n = Note::parse(text).unwrap();
        let s = note_state(&n);
        assert!(s.starts_with("Title: Kurs"));
        assert!(s.contains("声调 üben"));
        assert!(s.contains("me: Wie spricht man das aus?"));
        assert!(!s.contains("00:00:03") && !s.contains("ln:") && !s.contains("Alt"));
        let long = Note::parse(&"汉".repeat(20_000)).unwrap();
        assert!(note_state(&long).len() <= MAX_STATE_BYTES);
    }

    #[test]
    fn one_question_per_category() {
        let list = vec![
            Category { name: "Chinesisch-Kurs".into(), color: TagColor::Red, hint: String::new() },
            Category { name: "Arztagent".into(), color: TagColor::Blue, hint: "Ideen für neue Anforderungen".into() },
        ];
        let q = questions(&list);
        assert_eq!(q.len(), 2);
        assert_eq!(q[1].name, "c1");
        assert!(q[1].instructions.contains("\"Arztagent\" (Ideen für neue Anforderungen)"));
        assert_ne!(state_hash("a", &list), state_hash("a", &list[..1]));
    }
}
