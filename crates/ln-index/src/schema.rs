use rusqlite::Connection;

const MIGRATIONS: &[&str] = &[
    // v1 — SPEC §8.2
    "CREATE TABLE sources (
        id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL, kind TEXT NOT NULL,
        include_subdirs INTEGER NOT NULL DEFAULT 1, excludes TEXT, paused INTEGER NOT NULL DEFAULT 0,
        added_at INTEGER NOT NULL
    );
    CREATE TABLE files (
        id INTEGER PRIMARY KEY, source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
        path TEXT UNIQUE NOT NULL, mime TEXT, size INTEGER, mtime INTEGER, hash BLOB,
        title TEXT, lang TEXT, status TEXT NOT NULL,
        error TEXT, indexed_at INTEGER, parser_version INTEGER, embed_model TEXT
    );
    CREATE INDEX files_source ON files(source_id);
    CREATE TABLE chunks (
        id INTEGER PRIMARY KEY, file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
        ord INTEGER NOT NULL, text TEXT NOT NULL, lang TEXT, locator TEXT NOT NULL DEFAULT '{}',
        embed_model TEXT
    );
    CREATE INDEX chunks_file ON chunks(file_id);
    CREATE INDEX chunks_embed ON chunks(embed_model);
    CREATE VIRTUAL TABLE chunks_fts USING fts5(text, tokenize = 'unicode61 remove_diacritics 2');
    CREATE VIRTUAL TABLE chunks_fts_cjk USING fts5(text, tokenize = 'trigram');
    CREATE VIRTUAL TABLE chunks_vec USING vec0(embedding int8[256]);
    CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);",
];

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY);")?;
    let current: i64 = conn.query_row("SELECT COALESCE(MAX(version), 0) FROM schema_migrations", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let v = i as i64 + 1;
        if v > current {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute("INSERT INTO schema_migrations(version) VALUES (?1)", [v])?;
            tx.commit()?;
        }
    }
    Ok(())
}
