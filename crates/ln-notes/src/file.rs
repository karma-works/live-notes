use crate::{Note, Result};
use chrono::{DateTime, Datelike, FixedOffset, Timelike};
use ln_types::UiLanguage;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Writes a file atomically: temp file in the same directory, fsync, rename.
pub fn atomic_write(path: &Path, contents: &str) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(contents.as_bytes())?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Content hash used to detect external changes.
pub fn file_fingerprint(contents: &[u8]) -> [u8; 32] {
    *blake3::hash(contents).as_bytes()
}

/// Removes characters that are invalid or awkward in file names; keeps CJK.
pub fn sanitize_title(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { ' ' } else { c })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches(['.', ' ']);
    trimmed.chars().take(80).collect::<String>().trim().to_string()
}

/// `2026/10/2026-10-21 1430 <title>.md` relative to the notes folder (SPEC §8.1).
pub fn recording_file_name(created: &DateTime<FixedOffset>, title: Option<&str>, ui: UiLanguage) -> PathBuf {
    let fallback = match ui {
        UiLanguage::De => "Aufnahme",
        UiLanguage::En => "Recording",
    };
    let t = title.map(sanitize_title).filter(|t| !t.is_empty()).unwrap_or_else(|| fallback.to_string());
    PathBuf::from(format!("{:04}", created.year())).join(format!("{:02}", created.month())).join(format!(
        "{:04}-{:02}-{:02} {:02}{:02} {}.md",
        created.year(),
        created.month(),
        created.day(),
        created.hour(),
        created.minute(),
        t
    ))
}

/// Adds ` 2`, ` 3` … before the extension until the path is free (or equals `allow`).
pub fn unique_path(path: &Path, allow: Option<&Path>) -> PathBuf {
    if !path.exists() || Some(path) == allow {
        return path.to_path_buf();
    }
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("note");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("md");
    for i in 2.. {
        let p = path.with_file_name(format!("{stem} {i}.{ext}"));
        if !p.exists() || Some(p.as_path()) == allow {
            return p;
        }
    }
    unreachable!()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveOutcome {
    Saved,
    /// The file changed on disk since it was loaded; nothing was written.
    Conflict,
}

/// A note bound to a file, with the fingerprint of the version last read or written.
/// Used by the editor for auto-save and conflict detection (SPEC §5 F10).
#[derive(Debug, Clone)]
pub struct NoteFile {
    pub path: PathBuf,
    pub text: String,
    fingerprint: Option<[u8; 32]>,
}

impl NoteFile {
    pub fn open(path: &Path) -> Result<NoteFile> {
        let bytes = std::fs::read(path)?;
        let fingerprint = Some(file_fingerprint(&bytes));
        Ok(NoteFile { path: path.to_path_buf(), text: String::from_utf8_lossy(&bytes).into_owned(), fingerprint })
    }

    pub fn new(path: &Path, text: String) -> NoteFile {
        NoteFile { path: path.to_path_buf(), text, fingerprint: None }
    }

    pub fn parse(&self) -> Result<Note> {
        Note::parse(&self.text)
    }

    /// True if the file on disk differs from the version we know.
    pub fn changed_on_disk(&self) -> bool {
        match std::fs::read(&self.path) {
            Ok(bytes) => Some(file_fingerprint(&bytes)) != self.fingerprint,
            Err(_) => self.fingerprint.is_some(),
        }
    }

    /// Saves unless the file was changed externally; `force` overwrites anyway.
    pub fn save(&mut self, force: bool) -> Result<SaveOutcome> {
        if !force && self.fingerprint.is_some() && self.changed_on_disk() {
            return Ok(SaveOutcome::Conflict);
        }
        atomic_write(&self.path, &self.text)?;
        self.fingerprint = Some(file_fingerprint(self.text.as_bytes()));
        Ok(SaveOutcome::Saved)
    }

    /// Reloads from disk (the "take theirs" choice).
    pub fn reload(&mut self) -> Result<()> {
        *self = NoteFile::open(&self.path)?;
        Ok(())
    }

    /// Moves the file and keeps the binding.
    pub fn rename_to(&mut self, new_path: &Path) -> Result<()> {
        if let Some(d) = new_path.parent() {
            std::fs::create_dir_all(d)?;
        }
        if self.path.exists() {
            std::fs::rename(&self.path, new_path)?;
        }
        self.path = new_path.to_path_buf();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_sanitising() {
        let c = DateTime::parse_from_rfc3339("2026-10-21T14:30:12+02:00").unwrap();
        assert_eq!(recording_file_name(&c, None, UiLanguage::De), PathBuf::from("2026/10/2026-10-21 1430 Aufnahme.md"));
        assert_eq!(
            recording_file_name(&c, Some("Budget: Q1/Q2?"), UiLanguage::En),
            PathBuf::from("2026/10/2026-10-21 1430 Budget Q1 Q2.md")
        );
        assert_eq!(sanitize_title("  预算 会议  "), "预算 会议");
    }

    #[test]
    fn conflict_detection_and_unique_paths() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.md");
        std::fs::write(&p, "one").unwrap();
        let mut f = NoteFile::open(&p).unwrap();
        f.text = "two".into();
        assert_eq!(f.save(false).unwrap(), SaveOutcome::Saved);
        std::fs::write(&p, "external").unwrap();
        f.text = "three".into();
        assert_eq!(f.save(false).unwrap(), SaveOutcome::Conflict);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "external");
        assert_eq!(f.save(true).unwrap(), SaveOutcome::Saved);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "three");
        assert_eq!(unique_path(&p, None), dir.path().join("a 2.md"));
        assert_eq!(unique_path(&p, Some(&p)), p);
    }
}
