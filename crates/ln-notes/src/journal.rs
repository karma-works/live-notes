//! Crash safety for recordings (SPEC §5 F1): one JSON line per transcribed segment,
//! fsynced. Contains text only, never audio.

use crate::{Error, Result};
use ln_types::Segment;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JournalEntry {
    Start { note_id: String, note_path: PathBuf, created: String },
    Segment(Segment),
    /// A segment replaced after a language correction.
    Replace { index: usize, segment: Segment },
    End,
}

pub struct JournalWriter {
    path: PathBuf,
    file: File,
}

impl JournalWriter {
    pub fn create(dir: &Path, note_id: &str, note_path: &Path, created: &str) -> Result<JournalWriter> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("{note_id}.jsonl"));
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let mut w = JournalWriter { path, file };
        w.append(&JournalEntry::Start {
            note_id: note_id.into(),
            note_path: note_path.to_path_buf(),
            created: created.into(),
        })?;
        Ok(w)
    }

    pub fn append(&mut self, entry: &JournalEntry) -> Result<()> {
        let mut line = serde_json::to_string(entry).map_err(|e| Error::Journal(e.to_string()))?;
        line.push('\n');
        self.file.write_all(line.as_bytes())?;
        self.file.sync_data()?;
        Ok(())
    }

    /// Marks the recording as finished and removes the journal.
    pub fn finish(mut self) -> Result<()> {
        self.append(&JournalEntry::End)?;
        drop(self.file);
        std::fs::remove_file(&self.path)?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// An unfinished recording found at startup.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveredSession {
    pub journal: PathBuf,
    pub note_id: String,
    pub note_path: PathBuf,
    pub created: String,
    pub segments: Vec<Segment>,
}

/// Reads all journals without an `End` entry. Truncated last lines are ignored.
pub fn recover_sessions(dir: &Path) -> Result<Vec<RecoveredSession>> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return Ok(out) };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let f = File::open(&p)?;
        let mut session: Option<RecoveredSession> = None;
        let mut ended = false;
        for line in BufReader::new(f).lines() {
            let Ok(line) = line else { break };
            let Ok(e) = serde_json::from_str::<JournalEntry>(&line) else { continue };
            match e {
                JournalEntry::Start { note_id, note_path, created } => {
                    session = Some(RecoveredSession { journal: p.clone(), note_id, note_path, created, segments: vec![] })
                }
                JournalEntry::Segment(s) => {
                    if let Some(ss) = session.as_mut() {
                        ss.segments.push(s)
                    }
                }
                JournalEntry::Replace { index, segment } => {
                    if let Some(ss) = session.as_mut()
                        && index < ss.segments.len()
                    {
                        ss.segments[index] = segment;
                    }
                }
                JournalEntry::End => ended = true,
            }
        }
        match (session, ended) {
            (Some(s), false) => out.push(s),
            _ => {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ln_types::Channel;

    fn seg(t: &str) -> Segment {
        Segment { channel: Channel::Me, start_ms: 0, end_ms: 1000, lang: "de".into(), text: t.into(), lang_confidence: 1.0 }
    }

    #[test]
    fn recovers_unfinished_and_drops_finished() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = JournalWriter::create(dir.path(), "A", Path::new("/n/a.md"), "2026").unwrap();
        a.append(&JournalEntry::Segment(seg("eins"))).unwrap();
        a.append(&JournalEntry::Segment(seg("zwei"))).unwrap();
        a.append(&JournalEntry::Replace { index: 0, segment: seg("EINS") }).unwrap();
        // Simulate a torn write.
        std::fs::OpenOptions::new().append(true).open(a.path()).unwrap().write_all(b"{\"type\":\"seg").unwrap();
        let b = JournalWriter::create(dir.path(), "B", Path::new("/n/b.md"), "2026").unwrap();
        b.finish().unwrap();
        let rec = recover_sessions(dir.path()).unwrap();
        assert_eq!(rec.len(), 1);
        assert_eq!(rec[0].note_id, "A");
        assert_eq!(rec[0].segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), ["EINS", "zwei"]);
    }
}
