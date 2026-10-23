//! Background indexing (SPEC §5 F4): initial sync, file watching, embeddings.
//! Pauses while a recording runs (transcription has priority).

use crate::{Core, CoreEvent};
use crossbeam_channel::{Receiver, Sender, select, unbounded};
use ln_index::{FileStatus, FileUpsert, SourceKind};
use ln_ingest::{ExtractOptions, PARSER_VERSION, ScanOptions, ScannedFile, WatchEvent};
use parking_lot::Mutex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant, UNIX_EPOCH};

enum Cmd {
    ResyncAll,
    Kick,
    Shutdown,
}

#[derive(Default)]
pub struct IndexerHandle {
    tx: Mutex<Option<Sender<Cmd>>>,
}

impl IndexerHandle {
    fn send(&self, c: Cmd) {
        if let Some(tx) = self.tx.lock().as_ref() {
            let _ = tx.send(c);
        }
    }
    pub fn resync_all(&self) {
        self.send(Cmd::ResyncAll)
    }
    /// Wake up (e.g. the embedding model just arrived).
    pub fn kick(&self) {
        self.send(Cmd::Kick)
    }
    pub fn shutdown(&self) {
        self.send(Cmd::Shutdown)
    }
}

pub fn spawn(core: &Arc<Core>) {
    let (tx, rx) = unbounded();
    *core.indexer.tx.lock() = Some(tx);
    let _ = core.indexer.tx.lock().as_ref().unwrap().send(Cmd::ResyncAll);
    let weak = Arc::downgrade(core);
    std::thread::Builder::new().name("ln-indexer".into()).spawn(move || run(weak, rx)).expect("indexer thread");
}

struct Roots {
    /// (source id, root, include_subdirs, excludes)
    list: Vec<(i64, PathBuf, bool, Vec<String>)>,
}

fn run(weak: Weak<Core>, rx: Receiver<Cmd>) {
    let mut watcher = ln_ingest::FolderWatcher::new().ok();
    let watch_rx = watcher.as_ref().map(|w| w.events.clone()).unwrap_or_else(crossbeam_channel::never);
    let mut roots = Roots { list: vec![] };
    let mut watched: HashSet<PathBuf> = HashSet::new();
    let mut pending: HashSet<PathBuf> = HashSet::new();
    let mut last_event = Instant::now();
    let mut resync_needed = false;
    loop {
        let timeout = if pending.is_empty() { Duration::from_secs(3) } else { Duration::from_millis(400) };
        select! {
            recv(rx) -> m => match m {
                Ok(Cmd::ResyncAll) => resync_needed = true,
                Ok(Cmd::Kick) => {}
                Ok(Cmd::Shutdown) | Err(_) => return,
            },
            recv(watch_rx) -> ev => {
                if let Ok(ev) = ev {
                    let p = match ev { WatchEvent::Changed(p) | WatchEvent::Removed(p) => p };
                    pending.insert(p);
                    last_event = Instant::now();
                }
                continue;
            },
            default(timeout) => {},
        }
        let Some(core) = weak.upgrade() else { return };
        // Transcription has priority, also while a stopped recording is still being finished and
        // summarised: its note changes every minute and would be indexed and embedded again and
        // again. The changed files wait in `pending` and are indexed once afterwards.
        if core.recording_active() || !core.activities.lock().is_empty() {
            continue;
        }
        if resync_needed {
            resync_needed = false;
            roots = sources(&core);
            if let Some(w) = watcher.as_mut() {
                let want: HashSet<PathBuf> = roots.list.iter().map(|r| r.1.clone()).collect();
                for p in watched.difference(&want) {
                    w.unwatch(p);
                }
                for (_, p, rec, _) in &roots.list {
                    if !watched.contains(p) {
                        let _ = w.watch(p, *rec);
                    }
                }
                watched = want;
            }
            for (id, root, sub, ex) in roots.list.clone() {
                let opts = ScanOptions { include_subdirs: sub, excludes: ex, ..Default::default() };
                sync_root(&core, id, &root, &opts, &rx_peek(&rx));
                core.emit(CoreEvent::SourcesChanged);
            }
            core.emit(CoreEvent::NotesChanged);
        }
        if !pending.is_empty() && last_event.elapsed() > Duration::from_millis(800) {
            let notes_dir = core.notes_dir();
            let mut notes_touched = false;
            for p in pending.drain() {
                if p.starts_with(&notes_dir) && p.extension().is_some_and(|e| e == "md") {
                    notes_touched = true;
                    core.emit(CoreEvent::NoteFileChanged(p.clone()));
                }
                let Some((id, ..)) = roots.list.iter().find(|r| p.starts_with(&r.1)) else { continue };
                if p.is_file() {
                    if let Some(f) = scanned(&p) {
                        let _ = index_one(&core, *id, &f);
                    }
                } else if !p.exists() {
                    let _ = core.index.lock().remove_file(&p);
                }
            }
            if notes_touched {
                core.emit(CoreEvent::NotesChanged);
            }
            core.emit(CoreEvent::SourcesChanged);
        }
        embed_pending(&core, &rx);
    }
}

/// Stops long work when a new command arrives.
fn rx_peek(rx: &Receiver<Cmd>) -> impl Fn() -> bool + '_ {
    move || !rx.is_empty()
}

fn sources(core: &Arc<Core>) -> Roots {
    let s = core.settings();
    let notes = core.notes_dir();
    let mut list = Vec::new();
    let ix = core.index.lock();
    // Drop sources that are no longer configured.
    for row in ix.sources().unwrap_or_default() {
        let keep = row.path == notes || s.sources.iter().any(|x| x.path == row.path);
        if !keep {
            let _ = ix.remove_source(row.id);
        }
    }
    if let Ok(id) = ix.upsert_source(&notes, SourceKind::Notes) {
        list.push((id, notes.clone(), true, vec![]));
    }
    for src in s.sources.iter().filter(|x| !x.paused && x.path.exists()) {
        if src.path == notes {
            continue;
        }
        if let Ok(id) = ix.upsert_source(&src.path, SourceKind::Folder) {
            list.push((id, src.path.clone(), src.include_subdirs, src.excludes.clone()));
        }
    }
    Roots { list }
}

fn scanned(p: &Path) -> Option<ScannedFile> {
    let md = std::fs::metadata(p).ok()?;
    let mtime = md.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    ln_ingest::kind_of(p)?;
    Some(ScannedFile { path: p.to_path_buf(), size: md.len(), mtime })
}

fn sync_root(core: &Arc<Core>, source_id: i64, root: &Path, opts: &ScanOptions, cancel: &dyn Fn() -> bool) {
    let files = ln_ingest::scan(root, opts);
    let present: HashSet<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
    {
        let ix = core.index.lock();
        for old in ix.files_of_source(source_id).unwrap_or_default() {
            if !present.contains(&old) {
                let _ = ix.remove_file(&old);
            }
        }
    }
    for (i, f) in files.iter().enumerate() {
        if cancel() || core.recording_active() {
            break;
        }
        let _ = index_one(core, source_id, f);
        if i % 50 == 49 {
            core.emit(CoreEvent::SourcesChanged);
        }
    }
}

/// Like `ln_ingest::index_file`, but extracts outside the index lock so search stays responsive.
fn index_one(core: &Arc<Core>, source_id: i64, f: &ScannedFile) -> crate::Result<()> {
    let existing = core.index.lock().file(&f.path)?;
    if let Some(row) = &existing
        && row.size == f.size as i64
        && row.mtime == f.mtime
        && row.parser_version == PARSER_VERSION
        && matches!(row.status, FileStatus::Indexed | FileStatus::Skipped)
    {
        return Ok(());
    }
    let bytes = std::fs::read(&f.path)?;
    let hash = blake3_hash(&bytes);
    if let Some(row) = &existing
        && row.hash.as_deref() == Some(&hash[..])
        && row.parser_version == PARSER_VERSION
        && row.status == FileStatus::Indexed
    {
        core.index.lock().mark_file(source_id, &f.path, f.size as i64, f.mtime, FileStatus::Indexed, None)?;
        return Ok(());
    }
    let langs = ln_platform::ocr_languages(&core.settings().languages.mine);
    let opts = ExtractOptions { ocr_languages: langs, describe_image: None };
    let extracted = match ln_ingest::extract(&f.path, &opts) {
        Ok(e) => e,
        Err(ln_ingest::Error::Unsupported) => {
            core.index.lock().mark_file(source_id, &f.path, f.size as i64, f.mtime, FileStatus::Skipped, Some("unsupported"))?;
            return Ok(());
        }
        Err(e) => {
            core.index.lock().mark_file(source_id, &f.path, f.size as i64, f.mtime, FileStatus::Error, Some(&e.to_string()))?;
            return Ok(());
        }
    };
    let head: String = extracted.blocks.iter().take(5).map(|b| b.text.as_str()).collect::<Vec<_>>().join(" ");
    let lang = ln_ingest::detect_lang(&head);
    let chunks = ln_ingest::chunk_blocks(&extracted.blocks, lang.as_deref());
    let kind = ln_ingest::kind_of(&f.path);
    core.index.lock().upsert_file(&FileUpsert {
        source_id,
        path: &f.path,
        mime: kind.map(|k| k.mime()).unwrap_or("application/octet-stream"),
        size: f.size as i64,
        mtime: f.mtime,
        hash: &hash,
        title: &extracted.title,
        lang: lang.as_deref(),
        parser_version: PARSER_VERSION,
        chunks: &chunks,
    })?;
    Ok(())
}

fn blake3_hash(b: &[u8]) -> [u8; 32] {
    ln_notes::file_fingerprint(b)
}

fn embed_pending(core: &Arc<Core>, rx: &Receiver<Cmd>) {
    if !core.embedder_available() {
        return;
    }
    let model = core.settings().roles.embeddings.model;
    loop {
        if !rx.is_empty() || core.recording_active() {
            return;
        }
        let batch = core.index.lock().chunks_to_embed(&model, 16).unwrap_or_default();
        if batch.is_empty() {
            return;
        }
        let texts: Vec<String> = batch.iter().map(|(_, t)| t.clone()).collect();
        let vecs = match core.embed(&texts, false) {
            Ok(v) => v,
            Err(crate::ai::Unavailable::Server(e)) if is_too_large(&e) => {
                // One chunk is longer than the model's context (code, JSON, hashes have far more
                // tokens than the chunker estimates). Without this the same batch failed forever,
                // every few seconds, and nothing else got embedded.
                match embed_one_by_one(core, &texts) {
                    Some(v) => v,
                    None => return,
                }
            }
            Err(e) => {
                tracing::warn!("embedding unavailable: {e:?}");
                return;
            }
        };
        let ix = core.index.lock();
        for ((id, _), v) in batch.iter().zip(vecs) {
            if let Err(e) = ix.set_embedding(*id, &model, &v) {
                tracing::warn!("embedding {id}: {e}");
            }
        }
        let st = ix.stats().unwrap_or_default();
        drop(ix);
        core.emit(CoreEvent::IndexProgress { files: st.files, chunks: st.chunks, embedded: st.embedded });
    }
}

fn is_too_large(e: &str) -> bool {
    e.contains("too large") || e.contains("exceeds the available context") || e.contains("context size")
}

/// Embeds each text alone, cutting one that is too long in half until it fits (the start of a
/// chunk is what matters for finding it). `None` if the server is unavailable.
fn embed_one_by_one(core: &Arc<Core>, texts: &[String]) -> Option<Vec<Vec<f32>>> {
    let mut out = Vec::with_capacity(texts.len());
    for t in texts {
        let mut t = t.clone();
        loop {
            match core.embed(std::slice::from_ref(&t), false) {
                Ok(mut v) if v.len() == 1 => {
                    out.push(v.pop().unwrap());
                    break;
                }
                Err(crate::ai::Unavailable::Server(e)) if is_too_large(&e) && t.chars().count() > 64 => {
                    let half = t.chars().count() / 2;
                    t = t.chars().take(half).collect();
                }
                Ok(_) | Err(_) => {
                    tracing::warn!("embedding unavailable for a chunk of {} characters", t.chars().count());
                    return None;
                }
            }
        }
    }
    Some(out)
}
