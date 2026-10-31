//! Automatic summary after a recording (SPEC §5 F3): title, summary, decisions, tasks,
//! suggested additions to the user's notes; then the file is renamed after the title.

use crate::events::tr;
use crate::{Core, CoreEvent};
use ln_ai::prompts;
use ln_notes::{Note, NoteFile, SectionKind, recording_file_name, unique_path};
use ln_types::Channel;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Returns the note's path afterwards (it is renamed after the title), `None` if no summary
/// was written.
pub fn summarize(core: &Arc<Core>, path: &Path) -> Option<PathBuf> {
    if is_thought_note(path) {
        return consolidate(core, path);
    }
    let ui = core.ui_language();
    let role = match core.role(ln_types::ChatRole::Summary) {
        Ok(r) => r,
        Err(u) => {
            core.emit(CoreEvent::Notice(format!("{} {}", tr(ui, "Keine Zusammenfassung:", "No summary:"), core.unavailable_text(&u))));
            return None;
        }
    };
    let Ok(mut nf) = NoteFile::open(path) else { return None };
    let Ok(mut note) = nf.parse() else { return None };
    let lines = note.transcript_lines();
    if lines.is_empty() {
        return None;
    }
    let transcript: String = lines.iter().map(|l| format!("{}\n", l.format())).collect();
    let my_notes = note.section(&SectionKind::Notes).map(|s| s.body.clone()).unwrap_or_default();
    let s = core.settings();
    let out_lang =
        s.languages.summary_language.clone().or_else(|| note.main_language()).map(|l| l.to_string()).unwrap_or_else(|| ui.code().into());
    let labels: Vec<&str> =
        Channel::all().iter().flat_map(|c| [c.label(ln_types::UiLanguage::De), c.label(ln_types::UiLanguage::En)]).collect();
    let note_id = note.front.as_ref().and_then(|f| f.id.clone());
    let text =
        match core.chat_once(&role, prompts::summary(&transcript, &my_notes, &out_lang, &labels), true, "summary", note_id.as_deref()) {
            Ok(t) => t,
            Err(e) => {
                core.emit(CoreEvent::Notice(format!("{} {e}", tr(ui, "Zusammenfassung fehlgeschlagen:", "Summary failed:"))));
                return None;
            }
        };
    let Some(sum) = prompts::parse_summary(&text) else {
        core.emit(CoreEvent::Notice(tr(ui, "Die Zusammenfassung konnte nicht gelesen werden.", "Couldn't read the summary.")));
        return None;
    };
    // The file may have been edited meanwhile; re-read right before writing.
    if nf.changed_on_disk() {
        let _ = nf.reload();
        note = nf.parse().unwrap_or(note);
    }
    note.set_section(SectionKind::Summary, &sum.summary_markdown(), ui);
    if !sum.decisions.is_empty() {
        note.set_section(SectionKind::Decisions, &sum.decisions_markdown(), ui);
    }
    if !sum.tasks.is_empty() {
        note.set_section(SectionKind::Tasks, &sum.tasks_markdown(), ui);
    }
    // Summary, decisions, tasks go first.
    order_sections(&mut note);
    let manual = note.front.as_ref().is_some_and(|f| f.manual_name);
    if let Some(f) = note.front.as_mut()
        && !sum.title.trim().is_empty()
        && f.title.is_none()
    {
        f.title = Some(sum.title.trim().to_string());
    }
    nf.text = note.render();
    if nf.save(true).is_err() {
        return None;
    }
    let mut final_path = path.to_path_buf();
    if !manual && let Some(created) = note.front.as_ref().and_then(|f| f.created) {
        let name = recording_file_name(&created, Some(&sum.title), ui);
        let target = path.parent().unwrap_or(Path::new(".")).join(name.file_name().unwrap());
        let target = unique_path(&target, Some(path));
        if target != path && nf.rename_to(&target).is_ok() {
            final_path = target;
        }
    }
    if !sum.note_additions.is_empty() {
        core.additions.lock().insert(final_path.clone(), sum.note_additions.clone());
    }
    core.emit(CoreEvent::SummaryReady { path: final_path.clone() });
    core.emit(CoreEvent::NotesChanged);
    Some(final_path)
}

fn order_sections(note: &mut Note) {
    let rank = |k: &SectionKind| match k {
        SectionKind::Summary => 0,
        SectionKind::Decisions => 1,
        SectionKind::Tasks => 2,
        SectionKind::Notes => 3,
        SectionKind::Suggestions => 4,
        SectionKind::Other(_) => 5,
        SectionKind::Transcript => 6,
    };
    note.sections.sort_by_key(|s| rank(&s.kind));
}

/// A pending thought document keeps its source transcript until consolidation succeeds.
pub fn is_thought_note(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| Note::parse(&s).ok())
        .and_then(|n| n.front)
        .is_some_and(|f| f.extra.get("recording_mode").and_then(|v| v.as_str()) == Some("thoughts"))
}

fn consolidate(core: &Arc<Core>, path: &Path) -> Option<PathBuf> {
    let ui = core.ui_language();
    let fail = |reason: String| {
        core.emit(CoreEvent::Notice(format!(
            "{} {reason}",
            tr(
                ui,
                "Gedankenordnen fehlgeschlagen. Die Ausgangsaussagen bleiben für einen erneuten Versuch erhalten.",
                "Couldn't organize thoughts. The source text is kept so you can retry."
            )
        )));
    };
    let role = match core.role(ln_types::ChatRole::Summary) {
        Ok(r) => r,
        Err(u) => {
            fail(core.unavailable_text(&u));
            return None;
        }
    };
    let mut nf = NoteFile::open(path).ok()?;
    let mut note = nf.parse().ok()?;
    let lines = note.transcript_lines();
    if lines.is_empty() {
        return None;
    }
    let transcript = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
    let own = note.section(&SectionKind::Notes).map(|s| s.body.as_str()).unwrap_or("");
    let out_lang = core
        .settings()
        .languages
        .summary_language
        .or_else(|| note.main_language())
        .map(|l| l.to_string())
        .unwrap_or_else(|| ui.code().into());
    let id = note.front.as_ref().and_then(|f| f.id.as_deref());
    let response = match core.chat_once(&role, prompts::consolidate(&transcript, own, &out_lang), true, "consolidation", id) {
        Ok(t) => t,
        Err(e) => {
            fail(e);
            return None;
        }
    };
    let Some(doc) = prompts::parse_consolidation(&response) else {
        fail(tr(ui, "Ungültige Modellantwort.", "Invalid model response."));
        return None;
    };
    // Preserve edits made during inference; retry with the updated source instead of overwriting them.
    if nf.changed_on_disk() {
        fail(tr(ui, "Die Notiz wurde inzwischen bearbeitet. Bitte erneut versuchen.", "The note changed. Please retry."));
        return None;
    }
    let manual = note.front.as_ref().is_some_and(|f| f.manual_name);
    if let Some(f) = note.front.as_mut() {
        f.title = Some(doc.title.trim().into());
        f.extra.insert("consolidated".into(), true.into());
    }
    note.preamble = format!("# {}\n\n{}\n", doc.title.trim(), doc.document.trim());
    note.sections.clear();
    nf.text = note.render();
    match nf.save(false) {
        Ok(ln_notes::SaveOutcome::Saved) => {}
        Ok(ln_notes::SaveOutcome::Conflict) => {
            fail(tr(ui, "Die Notiz wurde inzwischen bearbeitet. Bitte erneut versuchen.", "The note changed. Please retry."));
            return None;
        }
        Err(e) => {
            fail(e.to_string());
            return None;
        }
    }
    let mut final_path = path.to_path_buf();
    if !manual && let Some(created) = note.front.as_ref().and_then(|f| f.created) {
        let name = recording_file_name(&created, Some(&doc.title), ui);
        let target = unique_path(&path.parent().unwrap_or(Path::new(".")).join(name.file_name().unwrap()), Some(path));
        if target != path && nf.rename_to(&target).is_ok() {
            final_path = target;
        }
    }
    core.emit(CoreEvent::NoteFileChanged(final_path.clone()));
    core.emit(CoreEvent::SummaryReady { path: final_path.clone() });
    core.emit(CoreEvent::NotesChanged);
    core.indexer.kick();
    Some(final_path)
}

impl Core {
    pub fn retry_consolidation(self: &Arc<Self>, path: &Path) {
        if self.recording_active() || !is_thought_note(path) {
            return;
        }
        {
            let mut activities = self.activities.lock();
            if activities.iter().any(|(p, _)| p == path) {
                return;
            }
            activities.push((path.to_path_buf(), crate::Activity::Summarizing));
        }
        self.emit(CoreEvent::ActivityChanged);
        let me = self.clone();
        let path = path.to_path_buf();
        std::thread::spawn(move || {
            let renamed = consolidate(&me, &path);
            me.set_activity(&path, None);
            if let Some(p) = renamed {
                me.classify_note(&p);
            }
        });
    }
}
