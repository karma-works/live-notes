use crate::{ExtractOptions, PARSER_VERSION, ScanOptions, ScannedFile, chunk_blocks, extract, kind_of, scan};
use ln_index::{FileStatus, FileUpsert, Index};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SyncReport {
    pub scanned: usize,
    pub indexed: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub errors: usize,
    pub skipped: usize,
}

/// Brings one source up to date: new and changed files are (re)indexed, deleted files removed.
/// Changes are found by size + mtime, confirmed by BLAKE3 hash (SPEC §5 F4).
pub fn sync_source(
    index: &mut Index,
    source_id: i64,
    root: &Path,
    scan_opts: &ScanOptions,
    opts: &ExtractOptions<'_>,
    cancel: &dyn Fn() -> bool,
) -> crate::Result<SyncReport> {
    let files = scan(root, scan_opts);
    let mut report = SyncReport { scanned: files.len(), ..Default::default() };
    let present: std::collections::HashSet<_> = files.iter().map(|f| f.path.clone()).collect();
    for old in index.files_of_source(source_id)? {
        if !present.contains(&old) {
            index.remove_file(&old)?;
            report.removed += 1;
        }
    }
    for f in &files {
        if cancel() {
            break;
        }
        match index_file(index, source_id, f, opts) {
            Ok(Outcome::Indexed) => report.indexed += 1,
            Ok(Outcome::Unchanged) => report.unchanged += 1,
            Ok(Outcome::Skipped) => report.skipped += 1,
            Err(e) => {
                tracing::debug!("index {}: {e}", f.path.display());
                report.errors += 1;
            }
        }
    }
    Ok(report)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Indexed,
    Unchanged,
    Skipped,
}

/// Indexes one file if it changed.
pub fn index_file(index: &mut Index, source_id: i64, f: &ScannedFile, opts: &ExtractOptions<'_>) -> crate::Result<Outcome> {
    let existing = index.file(&f.path)?;
    if let Some(row) = &existing
        && row.size == f.size as i64
        && row.mtime == f.mtime
        && row.parser_version == PARSER_VERSION
        && matches!(row.status, FileStatus::Indexed | FileStatus::Skipped)
    {
        return Ok(Outcome::Unchanged);
    }
    let bytes = std::fs::read(&f.path)?;
    let hash = blake3::hash(&bytes);
    if let Some(row) = &existing
        && row.hash.as_deref() == Some(hash.as_bytes().as_slice())
        && row.parser_version == PARSER_VERSION
        && row.status == FileStatus::Indexed
    {
        // Touched but not changed: just remember the new mtime.
        index.mark_file(source_id, &f.path, f.size as i64, f.mtime, FileStatus::Indexed, None)?;
        return Ok(Outcome::Unchanged);
    }
    let kind = kind_of(&f.path);
    let extracted = match extract(&f.path, opts) {
        Ok(e) => e,
        Err(crate::Error::Unsupported) => {
            index.mark_file(source_id, &f.path, f.size as i64, f.mtime, FileStatus::Skipped, Some("unsupported"))?;
            return Ok(Outcome::Skipped);
        }
        Err(e) => {
            index.mark_file(source_id, &f.path, f.size as i64, f.mtime, FileStatus::Error, Some(&e.to_string()))?;
            return Err(e);
        }
    };
    let all_text: String = extracted.blocks.iter().take(5).map(|b| b.text.as_str()).collect::<Vec<_>>().join(" ");
    let file_lang = crate::detect_lang(&all_text);
    let chunks = chunk_blocks(&extracted.blocks, file_lang.as_deref());
    index.upsert_file(&FileUpsert {
        source_id,
        path: &f.path,
        mime: kind.map(|k| k.mime()).unwrap_or("application/octet-stream"),
        size: f.size as i64,
        mtime: f.mtime,
        hash: hash.as_bytes(),
        title: &extracted.title,
        lang: file_lang.as_deref(),
        parser_version: PARSER_VERSION,
        chunks: &chunks,
    })?;
    Ok(Outcome::Indexed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ln_index::{SearchFilter, SourceKind};

    #[test]
    fn incremental_sync() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::write(root.join("a.md"), "# Angebot\nDer Preis gilt bis Ende Q1.").unwrap();
        std::fs::write(root.join("b.txt"), "Milch und Brot").unwrap();
        let mut ix = Index::open_in_memory().unwrap();
        let src = ix.upsert_source(root, SourceKind::Folder).unwrap();
        let opts = ExtractOptions::default();
        let r = sync_source(&mut ix, src, root, &ScanOptions::default(), &opts, &|| false).unwrap();
        assert_eq!((r.scanned, r.indexed), (2, 2));
        let hits = ix.search("Preis", None, &SearchFilter::default(), 5).unwrap();
        assert_eq!(hits[0].title, "Angebot");

        let r = sync_source(&mut ix, src, root, &ScanOptions::default(), &opts, &|| false).unwrap();
        assert_eq!(r.unchanged, 2);

        std::fs::write(root.join("b.txt"), "Käse und Wein und noch mehr").unwrap();
        std::fs::remove_file(root.join("a.md")).unwrap();
        let r = sync_source(&mut ix, src, root, &ScanOptions::default(), &opts, &|| false).unwrap();
        assert_eq!((r.indexed, r.removed), (1, 1));
        assert!(ix.search("Preis", None, &SearchFilter::default(), 5).unwrap().is_empty());
        assert_eq!(ix.search("Käse", None, &SearchFilter::default(), 5).unwrap().len(), 1);
    }
}
