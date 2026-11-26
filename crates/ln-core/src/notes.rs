//! Notes folder: listing, creating, renaming, tasks, suggested additions (SPEC §5 F3, F10).

use crate::{Core, CoreEvent, Result};
use chrono::Local;
use ln_notes::{Note, NoteFile, SectionKind, atomic_write, sanitize_title, unique_path};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct NoteEntry {
    pub path: PathBuf,
    pub title: String,
    /// Folder relative to the notes folder ("" for the top level).
    pub folder: String,
    pub modified: i64,
    pub created: Option<i64>,
    pub languages: Vec<String>,
    /// Recording notes have a transcript.
    pub is_recording: bool,
    pub labels: Vec<String>,
    /// The user's categories (front matter `categories:`), shown as coloured dots.
    pub categories: Vec<String>,
}

impl Core {
    /// All Markdown notes, newest first.
    pub fn list_notes(&self) -> Vec<NoteEntry> {
        let root = self.notes_dir();
        let files = ln_ingest::scan(&root, &ln_ingest::ScanOptions { skip_note_assets: true, ..Default::default() });
        let mut out: Vec<NoteEntry> = files
            .into_iter()
            .filter(|f| f.path.extension().is_some_and(|e| e == "md"))
            .map(|f| {
                let text = std::fs::read_to_string(&f.path).unwrap_or_default();
                let note = Note::parse(&text).unwrap_or_default();
                let stem = f.path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                let title = note.title().filter(|t| !t.trim().is_empty()).unwrap_or(stem);
                let folder =
                    f.path.parent().and_then(|p| p.strip_prefix(&root).ok()).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                let front = note.front.clone().unwrap_or_default();
                NoteEntry {
                    title,
                    folder,
                    modified: f.mtime,
                    created: front.created.map(|c| c.timestamp()),
                    languages: front.languages.iter().map(|l| l.to_string()).collect(),
                    // Clipped videos have a transcript too, but were not recorded here.
                    is_recording: note.section(&SectionKind::Transcript).is_some() && front.source_kind.is_none(),
                    labels: front.labels.clone(),
                    categories: front.categories.clone(),
                    path: f.path,
                }
            })
            .collect();
        out.sort_by_key(|n| std::cmp::Reverse(n.created.unwrap_or(n.modified)));
        out
    }

    /// Folders inside the notes folder (relative paths), for the note list.
    pub fn note_folders(&self) -> Vec<String> {
        let root = self.notes_dir();
        let mut out = Vec::new();
        fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if p.is_dir() && !name.starts_with('.') && name != "assets" {
                    // Year/month folders of recordings are not shown as user folders.
                    let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
                    if !(name.len() == 4 && name.chars().all(|c| c.is_ascii_digit())) {
                        out.push(rel);
                        walk(&p, root, out);
                    }
                }
            }
        }
        walk(&root, &root, &mut out);
        out.sort();
        out
    }

    /// `⌘N`: a new empty note named after date and time, in `folder` (relative) or the top level.
    pub fn create_note(&self, folder: Option<&str>) -> Result<PathBuf> {
        let dir = match folder {
            Some(f) if !f.is_empty() => self.notes_dir().join(f),
            _ => self.notes_dir(),
        };
        std::fs::create_dir_all(&dir)?;
        let name = Local::now().format("%Y-%m-%d %H%M").to_string();
        let path = unique_path(&dir.join(format!("{name}.md")), None);
        atomic_write(&path, "# \n")?;
        self.emit(CoreEvent::NotesChanged);
        Ok(path)
    }

    pub fn create_folder(&self, name: &str) -> Result<PathBuf> {
        let clean = sanitize_title(name);
        let p = self.notes_dir().join(if clean.is_empty() { "Neu".into() } else { clean });
        std::fs::create_dir_all(&p)?;
        self.emit(CoreEvent::NotesChanged);
        Ok(p)
    }

    /// Renames a note by hand: the name in the note list (front-matter title, or the first
    /// heading of a plain note) and the file. Recording notes get `manual_name` so the app
    /// never renames them again; their date prefix stays in the file name.
    pub fn rename_note(&self, path: &Path, title: &str) -> Result<PathBuf> {
        let clean = sanitize_title(title);
        if clean.is_empty() {
            return Ok(path.to_path_buf());
        }
        let mut nf = NoteFile::open(path)?;
        let mut note = nf.parse()?;
        note.set_title(title);
        nf.text = note.render();
        nf.save(true)?;
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let name = match note.front.as_ref().and_then(|f| f.created) {
            // `2026-10-21 1430 <title>` keeps recordings sorted by date in the Finder.
            Some(_) if is_dated(stem) => format!("{} {clean}.md", &stem[..15]),
            _ => format!("{clean}.md"),
        };
        let new = unique_path(&path.with_file_name(name), Some(path));
        if new != path {
            nf.rename_to(&new)?;
        }
        self.emit(CoreEvent::NotesChanged);
        Ok(new)
    }

    /// Manual notes named by date get the first heading as name once it exists (SPEC §5 F10).
    pub fn auto_name_note(&self, path: &Path) -> Option<PathBuf> {
        let stem = path.file_stem()?.to_str()?;
        if !is_dated(stem) {
            return None;
        }
        let text = std::fs::read_to_string(path).ok()?;
        let note = Note::parse(&text).ok()?;
        // Recordings are named by their summary; front matter that only holds categories
        // does not make a note a recording.
        if note.front.as_ref().is_some_and(|f| f.id.is_some() || f.title.is_some() || f.manual_name) {
            return None;
        }
        let title = note.title()?;
        let clean = sanitize_title(&title);
        if clean.is_empty() {
            return None;
        }
        let new = unique_path(&path.with_file_name(format!("{clean}.md")), Some(path));
        std::fs::rename(path, &new).ok()?;
        self.emit(CoreEvent::NotesChanged);
        Some(new)
    }

    /// Moves a note to another folder. Images it links relatively (`assets/…`) are copied
    /// along, so its links keep working; the originals stay, as other notes may use them too.
    pub fn move_note(&self, path: &Path, folder: &str) -> Result<PathBuf> {
        let dir = self.notes_dir().join(folder);
        std::fs::create_dir_all(&dir)?;
        let new = unique_path(&dir.join(path.file_name().unwrap_or_default()), None);
        if let (Some(old_dir), Ok(text)) = (path.parent(), std::fs::read_to_string(path))
            && old_dir != dir
        {
            copy_linked_images(&text, old_dir, &dir);
        }
        std::fs::rename(path, &new)?;
        self.emit(CoreEvent::NotesChanged);
        Ok(new)
    }

    /// Moves a note to the system trash would need platform code; v1.0 deletes after a
    /// confirmation in the UI.
    pub fn delete_note(&self, path: &Path) -> Result<()> {
        std::fs::remove_file(path)?;
        self.emit(CoreEvent::NotesChanged);
        Ok(())
    }

    /// Clicking a task checkbox in the reading view.
    pub fn toggle_task(&self, path: &Path, line: usize) -> Result<()> {
        let mut nf = NoteFile::open(path)?;
        if let Some(t) = ln_notes::toggle_task(&nf.text, line) {
            nf.text = t;
            nf.save(true)?;
            self.emit(CoreEvent::NoteFileChanged(path.to_path_buf()));
        }
        Ok(())
    }

    /// Copies a dropped image into `assets/` next to the note, named by its content
    /// (`assets/<blake3 prefix>.<ext>`), and returns the Markdown link `![<file stem>](assets/…)`.
    /// The same image dropped twice is stored once.
    pub fn import_image(&self, note: &Path, image: &Path) -> Result<String> {
        if !ln_notes::is_image_path(image) {
            return Err(crate::Error::User(format!("not an image: {}", image.display())));
        }
        let bytes = std::fs::read(image)?;
        let ext = image.extension().and_then(|e| e.to_str()).unwrap_or("png");
        let alt = image.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        self.import_image_bytes(note, &bytes, ext, alt)
    }

    /// Stores image bytes (e.g. pasted from the clipboard) like [`Core::import_image`].
    pub fn import_image_bytes(&self, note: &Path, bytes: &[u8], ext: &str, alt: &str) -> Result<String> {
        let dir = note.parent().unwrap_or(Path::new("."));
        let rel = ln_notes::store_asset(dir, bytes, ext)?;
        Ok(ln_notes::image_markdown(alt, &rel))
    }

    // ---- suggested additions (SPEC §5 F3: only accepted ones go into the file) ------------

    pub fn pending_additions(&self, path: &Path) -> Vec<String> {
        self.additions.lock().get(path).cloned().unwrap_or_default()
    }

    pub fn accept_addition(&self, path: &Path, i: usize) -> Result<()> {
        let Some(text) = self.take_addition(path, i) else { return Ok(()) };
        let mut nf = NoteFile::open(path)?;
        let mut note = nf.parse()?;
        let ui = self.ui_language();
        let body = note.section(&SectionKind::Notes).map(|s| s.body.trim_end().to_string()).unwrap_or_default();
        let new = if body.is_empty() { format!("- {text}\n") } else { format!("{body}\n- {text}\n") };
        note.set_section(SectionKind::Notes, &new, ui);
        nf.text = note.render();
        nf.save(true)?;
        self.emit(CoreEvent::NoteFileChanged(path.to_path_buf()));
        Ok(())
    }

    pub fn dismiss_addition(&self, path: &Path, i: usize) {
        self.take_addition(path, i);
        self.emit(CoreEvent::NoteFileChanged(path.to_path_buf()));
    }

    fn take_addition(&self, path: &Path, i: usize) -> Option<String> {
        let mut a = self.additions.lock();
        let v = a.get_mut(path)?;
        (i < v.len()).then(|| v.remove(i))
    }
}

/// Copies the images `text` links relatively from `from` to the same relative path in `to`.
/// Only files inside `assets/` are copied; existing files are kept (names are content hashes).
fn copy_linked_images(text: &str, from: &Path, to: &Path) {
    for target in ln_notes::image_targets(text) {
        let Some(rel) = ln_notes::local_target(&target) else { continue };
        let safe = rel.components().all(|c| matches!(c, std::path::Component::Normal(_)));
        if !safe || rel.components().next().is_none_or(|c| c.as_os_str() != ln_notes::ASSETS_DIR) {
            continue;
        }
        let (src, dest) = (from.join(&rel), to.join(&rel));
        if !src.is_file() || dest.exists() {
            continue;
        }
        if let Some(p) = dest.parent()
            && let Err(e) = std::fs::create_dir_all(p).and_then(|_| std::fs::copy(&src, &dest))
        {
            tracing::warn!("copy image {}: {e}", rel.display());
        }
    }
}

/// `2026-10-21 1430…`: named by date and time (new manual notes, recordings).
fn is_dated(stem: &str) -> bool {
    stem.len() >= 15
        && stem.is_char_boundary(15)
        && stem.as_bytes()[4] == b'-'
        && stem[..15].chars().filter(|c| c.is_ascii_digit()).count() == 12
}
