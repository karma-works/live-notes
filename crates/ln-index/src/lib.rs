//! The search index (SPEC §8.2, §5 F5). Everything here can be rebuilt from the files.
//!
//! Hybrid search: BM25 over two FTS5 tables (unicode61 for alphabetic scripts, trigram for
//! CJK) plus k-NN over int8 embeddings in `sqlite-vec`, merged with reciprocal rank fusion.

mod query;
mod schema;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Once;

pub use query::{fts_query, has_cjk, snippet};

/// Embedding dimension after Matryoshka truncation (SPEC §8.4).
pub const EMBED_DIM: usize = 256;
/// RRF constant.
pub const RRF_K: f64 = 60.0;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("embedding has {0} dimensions, expected at least {EMBED_DIM}")]
    Dim(usize),
}
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Notes,
    Folder,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRow {
    pub id: i64,
    pub path: PathBuf,
    pub kind: SourceKind,
    pub files: i64,
    pub indexed: i64,
    pub errors: i64,
    pub bytes: i64,
    pub last_indexed: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileStatus {
    Pending,
    Indexed,
    Error,
    Skipped,
}

impl FileStatus {
    fn as_str(self) -> &'static str {
        match self {
            FileStatus::Pending => "pending",
            FileStatus::Indexed => "indexed",
            FileStatus::Error => "error",
            FileStatus::Skipped => "skipped",
        }
    }
    fn parse(s: &str) -> FileStatus {
        match s {
            "indexed" => FileStatus::Indexed,
            "error" => FileStatus::Error,
            "skipped" => FileStatus::Skipped,
            _ => FileStatus::Pending,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileRow {
    pub id: i64,
    pub source_id: i64,
    pub path: PathBuf,
    pub size: i64,
    pub mtime: i64,
    pub hash: Option<Vec<u8>>,
    pub status: FileStatus,
    pub parser_version: i64,
}

/// A chunk ready to be stored. `locator` says where in the file it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewChunk {
    pub text: String,
    pub lang: Option<String>,
    pub locator: Locator,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Locator {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub t_ms: Option<u64>,
    /// Heading path like "Projekt > Budget".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heading: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FileUpsert<'a> {
    pub source_id: i64,
    pub path: &'a Path,
    pub mime: &'a str,
    pub size: i64,
    pub mtime: i64,
    pub hash: &'a [u8],
    pub title: &'a str,
    pub lang: Option<&'a str>,
    pub parser_version: i64,
    pub chunks: &'a [NewChunk],
}

#[derive(Debug, Clone, Default)]
pub struct SearchFilter {
    pub source_ids: Option<Vec<i64>>,
    pub mime_prefix: Option<String>,
    /// Only files modified at or after this unix time.
    pub since: Option<i64>,
    pub lang: Option<String>,
    pub exclude_paths: Vec<PathBuf>,
    /// Restrict to one file (e.g. "only this note").
    pub path_prefix: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchHit {
    pub chunk_id: i64,
    pub file_id: i64,
    pub source_id: i64,
    pub path: PathBuf,
    pub title: String,
    pub mime: String,
    pub text: String,
    pub snippet: String,
    /// Byte ranges in `snippet` to highlight.
    pub highlights: Vec<(usize, usize)>,
    pub locator: Locator,
    pub lang: Option<String>,
    pub score: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct IndexStats {
    pub files: i64,
    pub chunks: i64,
    pub embedded: i64,
}

pub struct Index {
    conn: Connection,
}

static VEC_INIT: Once = Once::new();

fn register_vec() {
    VEC_INIT.call_once(|| unsafe {
        #[allow(clippy::missing_transmute_annotations)]
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ())));
    });
}

impl Index {
    pub fn open(path: &Path) -> Result<Index> {
        register_vec();
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let conn = Connection::open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Index> {
        register_vec();
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Index> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        schema::migrate(&conn)?;
        Ok(Index { conn })
    }

    // ---- sources -------------------------------------------------------------------------

    pub fn upsert_source(&self, path: &Path, kind: SourceKind) -> Result<i64> {
        let kind_s = match kind {
            SourceKind::Notes => "notes",
            SourceKind::Folder => "folder",
        };
        self.conn.execute(
            "INSERT INTO sources(path, kind, added_at) VALUES (?1, ?2, strftime('%s','now'))
             ON CONFLICT(path) DO UPDATE SET kind = excluded.kind",
            params![path_str(path), kind_s],
        )?;
        Ok(self.conn.query_row("SELECT id FROM sources WHERE path = ?1", [path_str(path)], |r| r.get(0))?)
    }

    pub fn source_id(&self, path: &Path) -> Result<Option<i64>> {
        Ok(self.conn.query_row("SELECT id FROM sources WHERE path = ?1", [path_str(path)], |r| r.get(0)).optional()?)
    }

    /// Removes a source and all its index data. Files on disk are never touched.
    pub fn remove_source(&self, id: i64) -> Result<()> {
        let ids: Vec<i64> = {
            let mut st = self.conn.prepare("SELECT c.id FROM chunks c JOIN files f ON f.id = c.file_id WHERE f.source_id = ?1")?;
            st.query_map([id], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        self.delete_chunk_aux(&ids)?;
        self.conn.execute("DELETE FROM sources WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn sources(&self) -> Result<Vec<SourceRow>> {
        let mut st = self.conn.prepare(
            "SELECT s.id, s.path, s.kind,
                    COUNT(f.id),
                    SUM(CASE WHEN f.status = 'indexed' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN f.status = 'error' THEN 1 ELSE 0 END),
                    COALESCE(SUM(f.size), 0),
                    MAX(f.indexed_at)
             FROM sources s LEFT JOIN files f ON f.source_id = s.id
             GROUP BY s.id ORDER BY s.kind DESC, s.path",
        )?;
        let rows = st.query_map([], |r| {
            Ok(SourceRow {
                id: r.get(0)?,
                path: PathBuf::from(r.get::<_, String>(1)?),
                kind: if r.get::<_, String>(2)? == "notes" { SourceKind::Notes } else { SourceKind::Folder },
                files: r.get(3)?,
                indexed: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
                errors: r.get::<_, Option<i64>>(5)?.unwrap_or(0),
                bytes: r.get(6)?,
                last_indexed: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    // ---- files ---------------------------------------------------------------------------

    pub fn file(&self, path: &Path) -> Result<Option<FileRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, source_id, path, size, mtime, hash, status, parser_version FROM files WHERE path = ?1",
                [path_str(path)],
                |r| {
                    Ok(FileRow {
                        id: r.get(0)?,
                        source_id: r.get(1)?,
                        path: PathBuf::from(r.get::<_, String>(2)?),
                        size: r.get(3)?,
                        mtime: r.get(4)?,
                        hash: r.get(5)?,
                        status: FileStatus::parse(&r.get::<_, String>(6)?),
                        parser_version: r.get::<_, Option<i64>>(7)?.unwrap_or(0),
                    })
                },
            )
            .optional()?)
    }

    pub fn files_of_source(&self, source_id: i64) -> Result<Vec<PathBuf>> {
        let mut st = self.conn.prepare("SELECT path FROM files WHERE source_id = ?1")?;
        let v = st.query_map([source_id], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        Ok(v.into_iter().map(PathBuf::from).collect())
    }

    /// Inserts or replaces a file and all its chunks in one transaction.
    pub fn upsert_file(&mut self, f: &FileUpsert<'_>) -> Result<i64> {
        let tx = self.conn.transaction()?;
        let old: Vec<i64> = {
            let mut st = tx.prepare("SELECT c.id FROM chunks c JOIN files fi ON fi.id = c.file_id WHERE fi.path = ?1")?;
            st.query_map([path_str(f.path)], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        delete_chunk_aux_tx(&tx, &old)?;
        tx.execute(
            "INSERT INTO files(source_id, path, mime, size, mtime, hash, title, lang, status, error, indexed_at, parser_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'indexed', NULL, strftime('%s','now'), ?9)
             ON CONFLICT(path) DO UPDATE SET source_id = excluded.source_id, mime = excluded.mime, size = excluded.size,
               mtime = excluded.mtime, hash = excluded.hash, title = excluded.title, lang = excluded.lang,
               status = 'indexed', error = NULL, indexed_at = excluded.indexed_at, parser_version = excluded.parser_version",
            params![f.source_id, path_str(f.path), f.mime, f.size, f.mtime, f.hash, f.title, f.lang, f.parser_version],
        )?;
        let file_id: i64 = tx.query_row("SELECT id FROM files WHERE path = ?1", [path_str(f.path)], |r| r.get(0))?;
        tx.execute("DELETE FROM chunks WHERE file_id = ?1", [file_id])?;
        {
            let mut ins = tx.prepare("INSERT INTO chunks(file_id, ord, text, lang, locator) VALUES (?1, ?2, ?3, ?4, ?5)")?;
            let mut fts = tx.prepare("INSERT INTO chunks_fts(rowid, text) VALUES (?1, ?2)")?;
            let mut fts_cjk = tx.prepare("INSERT INTO chunks_fts_cjk(rowid, text) VALUES (?1, ?2)")?;
            for (i, c) in f.chunks.iter().enumerate() {
                let loc = serde_json::to_string(&c.locator).unwrap_or_else(|_| "{}".into());
                ins.execute(params![file_id, i as i64, c.text, c.lang, loc])?;
                let id = tx.last_insert_rowid();
                // Title is searchable too.
                let searchable = format!("{} {}", f.title, c.text);
                fts.execute(params![id, searchable])?;
                if has_cjk(&searchable) {
                    fts_cjk.execute(params![id, searchable])?;
                }
            }
        }
        tx.commit()?;
        Ok(file_id)
    }

    pub fn mark_file(&self, source_id: i64, path: &Path, size: i64, mtime: i64, status: FileStatus, error: Option<&str>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO files(source_id, path, size, mtime, status, error, indexed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, strftime('%s','now'))
             ON CONFLICT(path) DO UPDATE SET status = excluded.status, error = excluded.error,
               size = excluded.size, mtime = excluded.mtime, indexed_at = excluded.indexed_at",
            params![source_id, path_str(path), size, mtime, status.as_str(), error],
        )?;
        Ok(())
    }

    pub fn remove_file(&self, path: &Path) -> Result<()> {
        let ids: Vec<i64> = {
            let mut st = self.conn.prepare("SELECT c.id FROM chunks c JOIN files f ON f.id = c.file_id WHERE f.path = ?1")?;
            st.query_map([path_str(path)], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        self.delete_chunk_aux(&ids)?;
        self.conn.execute("DELETE FROM files WHERE path = ?1", [path_str(path)])?;
        Ok(())
    }

    fn delete_chunk_aux(&self, ids: &[i64]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        delete_chunk_aux_tx(&tx, ids)?;
        tx.commit()?;
        Ok(())
    }

    // ---- embeddings ----------------------------------------------------------------------

    /// Chunks without an embedding from `model`, with the document prompt prefix (SPEC §8.4).
    pub fn chunks_to_embed(&self, model: &str, limit: usize) -> Result<Vec<(i64, String)>> {
        let mut st = self.conn.prepare(
            "SELECT c.id, COALESCE(f.title, ''), c.text, c.locator FROM chunks c JOIN files f ON f.id = c.file_id
             WHERE c.embed_model IS NULL OR c.embed_model != ?1 ORDER BY f.mtime DESC LIMIT ?2",
        )?;
        let rows = st.query_map(params![model, limit as i64], |r| {
            let title: String = r.get(1)?;
            let text: String = r.get(2)?;
            let loc: Locator = serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default();
            let t = match loc.heading {
                Some(h) if !h.is_empty() => format!("{title} > {h}"),
                _ => title,
            };
            let t = if t.is_empty() { "none".to_string() } else { t };
            Ok((r.get(0)?, format!("title: {t} | text: {text}")))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Stores an embedding: truncated to 256 dims, normalised, int8-quantised.
    pub fn set_embedding(&self, chunk_id: i64, model: &str, embedding: &[f32]) -> Result<()> {
        let q = quantize(embedding)?;
        self.conn.execute("DELETE FROM chunks_vec WHERE rowid = ?1", [chunk_id])?;
        self.conn.execute("INSERT INTO chunks_vec(rowid, embedding) VALUES (?1, vec_int8(?2))", params![chunk_id, q])?;
        self.conn.execute("UPDATE chunks SET embed_model = ?1 WHERE id = ?2", params![model, chunk_id])?;
        Ok(())
    }

    pub fn stats(&self) -> Result<IndexStats> {
        Ok(IndexStats {
            files: self.conn.query_row("SELECT COUNT(*) FROM files WHERE status = 'indexed'", [], |r| r.get(0))?,
            chunks: self.conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?,
            embedded: self.conn.query_row("SELECT COUNT(*) FROM chunks WHERE embed_model IS NOT NULL", [], |r| r.get(0))?,
        })
    }

    // ---- search --------------------------------------------------------------------------

    /// Hybrid search. `query_embedding` is the embedding of the query (with the query prompt);
    /// without it, only full-text search runs.
    pub fn search(&self, text: &str, query_embedding: Option<&[f32]>, filter: &SearchFilter, limit: usize) -> Result<Vec<SearchHit>> {
        let pool = (limit * 4).max(40);
        let mut ranked: Vec<Vec<i64>> = Vec::new();
        if let Some(q) = fts_query(text) {
            ranked.push(self.fts_ids("chunks_fts", &q, pool)?);
        }
        if has_cjk(text) {
            ranked.push(self.cjk_ids(text, pool)?);
        }
        if let Some(e) = query_embedding {
            ranked.push(self.vec_ids(e, pool)?);
        }
        let mut scores: HashMap<i64, f64> = HashMap::new();
        for list in &ranked {
            for (rank, id) in list.iter().enumerate() {
                *scores.entry(*id).or_default() += 1.0 / (RRF_K + rank as f64 + 1.0);
            }
        }
        let mut ids: Vec<(i64, f64)> = scores.into_iter().collect();
        ids.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut hits = Vec::new();
        for (id, score) in ids {
            if let Some(h) = self.hit(id, score, text)?
                && filter_ok(&h, filter, &self.conn)?
            {
                hits.push(h);
                if hits.len() >= limit {
                    break;
                }
            }
        }
        Ok(hits)
    }

    fn fts_ids(&self, table: &str, q: &str, n: usize) -> Result<Vec<i64>> {
        let sql = format!("SELECT rowid FROM {table} WHERE {table} MATCH ?1 ORDER BY bm25({table}) LIMIT ?2");
        let mut st = self.conn.prepare(&sql)?;
        let r = st.query_map(params![q, n as i64], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>();
        // A malformed query must not break search; it just yields no FTS hits.
        Ok(r.unwrap_or_default())
    }

    fn cjk_ids(&self, text: &str, n: usize) -> Result<Vec<i64>> {
        let runs = query::cjk_runs(text);
        let mut out = Vec::new();
        for run in runs {
            if run.chars().count() >= 3 {
                let q = format!("\"{}\"", run.replace('"', ""));
                out.extend(self.fts_ids("chunks_fts_cjk", &q, n)?);
            } else {
                // Trigram needs three characters; shorter runs use a substring scan.
                let mut st = self.conn.prepare("SELECT rowid FROM chunks_fts_cjk WHERE text LIKE ?1 LIMIT ?2")?;
                let like = format!("%{run}%");
                out.extend(st.query_map(params![like, n as i64], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?);
            }
        }
        out.dedup();
        Ok(out)
    }

    fn vec_ids(&self, e: &[f32], n: usize) -> Result<Vec<i64>> {
        let q = quantize(e)?;
        let mut st = self.conn.prepare("SELECT rowid FROM chunks_vec WHERE embedding MATCH vec_int8(?1) AND k = ?2 ORDER BY distance")?;
        Ok(st.query_map(params![q, n as i64], |r| r.get(0))?.collect::<Result<_, _>>()?)
    }

    fn hit(&self, chunk_id: i64, score: f64, query: &str) -> Result<Option<SearchHit>> {
        self.conn
            .query_row(
                "SELECT c.id, f.id, f.source_id, f.path, COALESCE(f.title, ''), COALESCE(f.mime, ''), c.text, c.locator, c.lang
                 FROM chunks c JOIN files f ON f.id = c.file_id WHERE c.id = ?1",
                [chunk_id],
                |r| {
                    let text: String = r.get(6)?;
                    let (snip, hl) = snippet(&text, query, 220);
                    Ok(SearchHit {
                        chunk_id: r.get(0)?,
                        file_id: r.get(1)?,
                        source_id: r.get(2)?,
                        path: PathBuf::from(r.get::<_, String>(3)?),
                        title: r.get(4)?,
                        mime: r.get(5)?,
                        locator: serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or_default(),
                        lang: r.get(8)?,
                        text,
                        snippet: snip,
                        highlights: hl,
                        score,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }
}

/// Keeps the best hit per file, preserving order.
pub fn group_by_file(hits: Vec<SearchHit>) -> Vec<SearchHit> {
    let mut seen = std::collections::HashSet::new();
    hits.into_iter().filter(|h| seen.insert(h.file_id)).collect()
}

fn filter_ok(h: &SearchHit, f: &SearchFilter, conn: &Connection) -> Result<bool> {
    if let Some(ids) = &f.source_ids
        && !ids.contains(&h.source_id)
    {
        return Ok(false);
    }
    if let Some(m) = &f.mime_prefix
        && !h.mime.starts_with(m.as_str())
    {
        return Ok(false);
    }
    if let Some(l) = &f.lang
        && h.lang.as_deref() != Some(l.as_str())
    {
        return Ok(false);
    }
    if f.exclude_paths.iter().any(|p| p == &h.path) {
        return Ok(false);
    }
    if let Some(p) = &f.path_prefix
        && !h.path.starts_with(p)
    {
        return Ok(false);
    }
    if let Some(since) = f.since {
        let mtime: i64 = conn.query_row("SELECT mtime FROM files WHERE id = ?1", [h.file_id], |r| r.get(0))?;
        if mtime < since {
            return Ok(false);
        }
    }
    Ok(true)
}

fn delete_chunk_aux_tx(tx: &rusqlite::Transaction<'_>, ids: &[i64]) -> Result<()> {
    let mut a = tx.prepare("DELETE FROM chunks_fts WHERE rowid = ?1")?;
    let mut b = tx.prepare("DELETE FROM chunks_fts_cjk WHERE rowid = ?1")?;
    let mut c = tx.prepare("DELETE FROM chunks_vec WHERE rowid = ?1")?;
    for id in ids {
        a.execute([id])?;
        b.execute([id])?;
        c.execute([id])?;
    }
    Ok(())
}

/// Matryoshka truncation to 256 dims, L2 normalisation, int8 quantisation.
pub fn quantize(e: &[f32]) -> Result<Vec<u8>> {
    if e.len() < EMBED_DIM {
        return Err(Error::Dim(e.len()));
    }
    let v = &e[..EMBED_DIM];
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
    Ok(v.iter().map(|x| ((x / norm) * 127.0).round().clamp(-127.0, 127.0) as i8 as u8).collect())
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests;
