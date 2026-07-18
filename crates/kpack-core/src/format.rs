//! `.kpack` SQLite format (§1.1) — schema, open/create, typed doc/chunk I/O,
//! and the vec0 (dense) + fts5 (lexical) dual-lane queries this milestone's
//! risk slice exists to prove.
//!
//! ## Schema
//! - `docs(id, title, source_type, sha256, source_path, source_size,
//!   source_mtime, extraction_quality, added_at)` — one row per source
//!   document (spec §1.1). Only `title`, `sha256`, `added_at` are NOT NULL;
//!   the rest are populated by later slices (`extraction_quality` by §3.2)
//!   or are inherently absent for curated packs (`source_path` — see below).
//! - `chunks(id, doc_id, section_path, locator, text, prefix, token_count)`
//!   — one row per retrievable unit. `section_path` and `locator` are
//!   **NOT NULL** (D6): citations are load-bearing, not optional metadata.
//!   A root/top-level chunk with no heading ancestry uses
//!   `section_path = ""`, never NULL. `prefix` is a short human-readable
//!   breadcrumb prepended in citations (e.g. "Chapter 2 > Setup"), distinct
//!   from `section_path`'s machine-oriented addressing; it defaults to `""`.
//!   `token_count` is produced by the tokenizer (K4/K5) — this slice adds
//!   the column/field only, callers supply the value.
//!
//!   Deviation from spec §1.1: the spec declares `prefix TEXT DEFAULT ''`
//!   (nullable); this schema uses `prefix TEXT NOT NULL DEFAULT ''`. The
//!   spec's "may be empty" means empty string, not NULL, so NOT NULL avoids
//!   an extra null-handling case for no loss of expressiveness — the one
//!   intentional deviation in this module.
//! - `vec` — a `vec0` virtual table, one dense int8 embedding row per chunk,
//!   with an explicit `chunk_id INTEGER PRIMARY KEY` column (spec §1.1) —
//!   not an implicit `rowid` alias. Column width (`dims`) is a construction
//!   parameter (embedder-dependent), not hardcoded; it's persisted as the
//!   manifest key `embedding_dims` on create (spec §1.2) and read back from
//!   there on `open`, so `insert_embedding` can validate vector length
//!   without re-deriving it from vec0's declared type.
//! - `fts` — an `fts5` external-content table over `chunks`
//!   (`content='chunks', content_rowid='id'`), indexing `text`, `prefix`,
//!   `section_path`, tokenized `porter unicode61`. External-content tables
//!   don't auto-sync on writes to the base table, so `insert_chunk` inserts
//!   into both explicitly.
//!   TODO(§later): no delete/update path exists yet. When one is added,
//!   external-content FTS5 can't reconstruct old column values from the
//!   index alone, so any delete/update MUST issue the fts5 `'delete'`
//!   special-insert (with the old values) before removing/changing the
//!   `chunks` row, or `'rebuild'` afterward — otherwise the index silently
//!   desyncs from `chunks` without erroring.
//! - `manifest(key, value)` — flat key/value store; `schema_version` and
//!   `embedding_dims` are written on create.
//!
//! ## API split
//! `insert_chunk` writes the chunk row + its fts row (fts5 indexing has no
//! separate identity from the chunk itself — every chunk is always
//! full-text-searchable). Embeddings are a separate `insert_embedding` call:
//! a chunk can exist (and be fts-searchable) before an embedder has run over
//! it, since the two lanes are populated by different pipeline stages in the
//! eventual `build_pack` (K8) — keeping them separable here avoids forcing
//! that ordering prematurely.

use rusqlite::ffi::sqlite3_auto_extension;
use rusqlite::{params, Connection, OptionalExtension};
use sqlite_vec::sqlite3_vec_init;
use std::fmt;
use std::path::Path;
use std::sync::Once;

/// Current on-disk schema version. Bump on any incompatible schema change;
/// K2's load-time gate compares this against a pack's stored value.
pub const SCHEMA_VERSION: i64 = 1;

static VEC_INIT: Once = Once::new();

/// Register sqlite-vec's `vec0` module as a SQLite auto-extension, once per
/// process. After this call, every `rusqlite::Connection` opened in this
/// process (bundled SQLite) has `vec0` available — no per-connection
/// `load_extension` call needed, and no dynamic library on disk.
fn register_vec() {
    VEC_INIT.call_once(|| unsafe {
        sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite3_vec_init as *const (),
        )));
    });
}

/// A `.kpack` document/chunk store — a thin typed wrapper over one SQLite
/// connection to a `.kpack` file. `dims` is the pack's `vec0` embedding
/// column width, cached at open/create time so `insert_embedding` can
/// validate vector length locally (Fix 5) instead of letting vec0 reject a
/// mismatch opaquely.
pub struct Pack {
    conn: Connection,
    dims: usize,
}

/// One source document (spec §1.1). `id` is ignored by `insert_doc` (SQLite
/// assigns the rowid); it's populated on read. `title`, `sha256`, and
/// `added_at` are required; everything else is nullable — `source_path` is
/// personal-packs-only (curated packs store NULL, and it's never logged),
/// `extraction_quality` is populated by a later slice (§3.2), and
/// `source_size`/`source_mtime` back the staleness pre-check (§3.6).
#[derive(Debug, Clone, PartialEq)]
pub struct Doc {
    pub id: i64,
    pub title: String,
    pub source_type: Option<String>,
    pub sha256: String,
    pub source_path: Option<String>,
    pub source_size: Option<i64>,
    pub source_mtime: Option<String>,
    pub extraction_quality: Option<f64>,
    pub added_at: String,
}

/// One retrievable chunk. `id` is ignored by `insert_chunk` (SQLite assigns
/// the rowid) and populated on read. `section_path`/`locator` are `String`,
/// not `Option<String>` — D6 requires citations to always resolve; a root
/// chunk with no section ancestry sets `section_path = String::new()`.
/// `token_count` is supplied by the caller (the tokenizer lands in K4/K5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub id: i64,
    pub doc_id: i64,
    pub section_path: String,
    pub locator: String,
    pub prefix: String,
    pub text: String,
    pub token_count: i64,
}

/// Errors from opening/reading/writing a `.kpack`. Hand-rolled (no
/// `thiserror` in this repo) — small on purpose.
#[derive(Debug)]
pub enum Error {
    Sqlite(rusqlite::Error),
    Schema(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Sqlite(e) => write!(f, "sqlite error: {e}"),
            Error::Schema(msg) => write!(f, "schema error: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Sqlite(e)
    }
}

type Result<T> = std::result::Result<T, Error>;

impl Pack {
    /// Open a `.kpack` at `path`, creating it (and applying the full schema)
    /// if it doesn't exist yet. `dims` sets the `vec0` embedding column
    /// width for a *new* pack; ignored when opening an existing one (its
    /// vec0 table's DDL is already stored in the file — SQLite reconstructs
    /// virtual-table state from `sqlite_master` on connect). Validating
    /// `dims` against an existing pack's actual width is K2's load-time
    /// gate, not this constructor's job.
    pub fn open_or_create<P: AsRef<Path>>(path: P, dims: usize) -> Result<Pack> {
        register_vec();
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let dims = if !schema_exists(&conn)? {
            create_schema(&conn, dims)?;
            dims
        } else {
            read_dims(&conn)?
        };
        Ok(Pack { conn, dims })
    }

    /// Open an existing `.kpack`. Does not create or modify schema; fails
    /// with `Error::Schema` if the file has no `manifest` table (not a
    /// `.kpack`, or a fresh empty SQLite file).
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Pack> {
        register_vec();
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        if !schema_exists(&conn)? {
            return Err(Error::Schema(
                "not a .kpack: manifest table missing".to_string(),
            ));
        }
        let dims = read_dims(&conn)?;
        Ok(Pack { conn, dims })
    }

    /// Insert a doc row. Returns the assigned id.
    pub fn insert_doc(&self, doc: &Doc) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO docs (
                title, source_type, sha256, source_path,
                source_size, source_mtime, extraction_quality, added_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                doc.title,
                doc.source_type,
                doc.sha256,
                doc.source_path,
                doc.source_size,
                doc.source_mtime,
                doc.extraction_quality,
                doc.added_at,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Read a doc by id.
    pub fn get_doc(&self, id: i64) -> Result<Option<Doc>> {
        self.conn
            .query_row(
                "SELECT id, title, source_type, sha256, source_path,
                        source_size, source_mtime, extraction_quality, added_at
                 FROM docs WHERE id = ?1",
                params![id],
                |row| {
                    Ok(Doc {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        source_type: row.get(2)?,
                        sha256: row.get(3)?,
                        source_path: row.get(4)?,
                        source_size: row.get(5)?,
                        source_mtime: row.get(6)?,
                        extraction_quality: row.get(7)?,
                        added_at: row.get(8)?,
                    })
                },
            )
            .optional()
            .map_err(Error::from)
    }

    /// Insert a chunk row AND its `fts5` row (external-content tables don't
    /// auto-sync — see the module doc comment). Does NOT write a `vec0` row;
    /// call `insert_embedding` separately once an embedder has run.
    pub fn insert_chunk(&self, chunk: &Chunk) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO chunks (doc_id, section_path, locator, prefix, text, token_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                chunk.doc_id,
                chunk.section_path,
                chunk.locator,
                chunk.prefix,
                chunk.text,
                chunk.token_count,
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.conn.execute(
            "INSERT INTO fts (rowid, text, prefix, section_path)
             VALUES (?1, ?2, ?3, ?4)",
            params![id, chunk.text, chunk.prefix, chunk.section_path],
        )?;
        Ok(id)
    }

    /// Read a chunk by id.
    pub fn get_chunk(&self, id: i64) -> Result<Option<Chunk>> {
        self.conn
            .query_row(
                "SELECT id, doc_id, section_path, locator, prefix, text, token_count
                 FROM chunks WHERE id = ?1",
                params![id],
                |row| {
                    Ok(Chunk {
                        id: row.get(0)?,
                        doc_id: row.get(1)?,
                        section_path: row.get(2)?,
                        locator: row.get(3)?,
                        prefix: row.get(4)?,
                        text: row.get(5)?,
                        token_count: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(Error::from)
    }

    /// Insert a chunk's dense int8 embedding into the `vec0` lane. `chunk_id`
    /// must already exist in `chunks` (not enforced by SQLite — vec0 has no
    /// foreign-key support — so this is a build-pipeline invariant, not a
    /// schema one). `vector`'s length must equal the pack's `dims`, checked
    /// here (Fix 5) so a mismatch fails with a clear `Error::Schema` instead
    /// of an opaque vec0 rejection.
    pub fn insert_embedding(&self, chunk_id: i64, vector: &[i8]) -> Result<()> {
        if vector.len() != self.dims {
            return Err(Error::Schema(format!(
                "embedding length {} does not match pack dims {}",
                vector.len(),
                self.dims
            )));
        }
        let blob = i8_slice_to_blob(vector);
        self.conn.execute(
            "INSERT INTO vec (chunk_id, embedding) VALUES (?1, vec_int8(?2))",
            params![chunk_id, blob],
        )?;
        Ok(())
    }

    /// K-nearest-neighbor search over the `vec0` dense lane. Returns
    /// `(chunk_id, distance)` pairs, nearest first — just enough to prove
    /// the dense lane is queryable; full retrieval fusion is §4.
    pub fn vec_search(&self, query: &[i8], k: usize) -> Result<Vec<(i64, f32)>> {
        let blob = i8_slice_to_blob(query);
        let mut stmt = self.conn.prepare(
            "SELECT chunk_id, distance FROM vec
             WHERE embedding MATCH vec_int8(?1) AND k = ?2",
        )?;
        let rows = stmt
            .query_map(params![blob, k as i64], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, f32>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Full-text search over the `fts5` lexical lane. Returns matching
    /// `chunk_id`s ranked by relevance (best first), capped at `k` — just
    /// enough to prove the lexical lane is queryable; full retrieval fusion
    /// is §4.
    pub fn fts_search(&self, query: &str, k: usize) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT rowid FROM fts WHERE fts MATCH ?1 ORDER BY rank LIMIT ?2")?;
        let rows = stmt
            .query_map(params![query, k as i64], |row| row.get::<_, i64>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Set a manifest key/value pair (upsert).
    pub fn manifest_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO manifest (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Read a manifest value by key.
    pub fn manifest_get(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM manifest WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(Error::from)
    }
}

/// True if this connection's database already has a `manifest` table — the
/// signal that schema has already been applied (a fresh/empty SQLite file
/// has no tables at all).
fn schema_exists(conn: &Connection) -> Result<bool> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='manifest')",
        [],
        |row| row.get(0),
    )?;
    Ok(exists)
}

/// Read the pack's `vec0` embedding width back from the manifest key
/// `embedding_dims` (written by `create_schema`). Any existing `.kpack`
/// must have this key — its absence means the file predates this slice's
/// schema or is otherwise malformed, which is a `Schema` error, not a panic.
fn read_dims(conn: &Connection) -> Result<usize> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM manifest WHERE key = 'embedding_dims'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    match raw {
        Some(s) => s
            .parse::<usize>()
            .map_err(|e| Error::Schema(format!("invalid embedding_dims manifest value: {e}"))),
        None => Err(Error::Schema(
            "manifest missing embedding_dims key".to_string(),
        )),
    }
}

/// Apply the full §1.1 schema (docs, chunks, vec, fts, manifest) to a fresh
/// connection, then record `schema_version` and `embedding_dims`.
fn create_schema(conn: &Connection, dims: usize) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE docs (
            id INTEGER PRIMARY KEY,
            title TEXT NOT NULL,
            source_type TEXT,
            sha256 TEXT NOT NULL,
            source_path TEXT,
            source_size INTEGER,
            source_mtime TEXT,
            extraction_quality REAL,
            added_at TEXT NOT NULL
        );

        CREATE TABLE chunks (
            id INTEGER PRIMARY KEY,
            doc_id INTEGER NOT NULL REFERENCES docs(id),
            section_path TEXT NOT NULL,
            locator TEXT NOT NULL,
            prefix TEXT NOT NULL DEFAULT '',
            text TEXT NOT NULL,
            token_count INTEGER NOT NULL
        );

        CREATE TABLE manifest (
            key TEXT PRIMARY KEY,
            value TEXT
        );

        CREATE VIRTUAL TABLE fts USING fts5(
            text, prefix, section_path,
            content='chunks', content_rowid='id',
            tokenize='porter unicode61'
        );",
    )?;

    // vec0's column width is a per-pack parameter, so this DDL is formatted
    // rather than a static string literal. `dims` is a `usize` (Display
    // emits only ASCII digits), so there is no SQL-injection surface here.
    let vec_ddl = format!(
        "CREATE VIRTUAL TABLE vec USING vec0(chunk_id INTEGER PRIMARY KEY, embedding int8[{dims}])"
    );
    conn.execute_batch(&vec_ddl)?;

    conn.execute(
        "INSERT INTO manifest (key, value) VALUES ('schema_version', ?1)",
        params![SCHEMA_VERSION.to_string()],
    )?;
    conn.execute(
        "INSERT INTO manifest (key, value) VALUES ('embedding_dims', ?1)",
        params![dims.to_string()],
    )?;
    Ok(())
}

/// Pack a `&[i8]` embedding into the raw byte blob `vec_int8()` expects (one
/// byte per element, same bit pattern as the source i8 — `as u8` is a
/// lossless reinterpret at this width, not a truncating cast).
fn i8_slice_to_blob(vector: &[i8]) -> Vec<u8> {
    vector.iter().map(|&b| b as u8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Mirrors `src-tauri/src/inference.rs`'s `unique_dir` temp-path helper
    /// (this repo hand-rolls temp dirs instead of depending on `tempfile`).
    fn unique_dir(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "cleophis-kpack-test-{}-{}-{}",
            std::process::id(),
            unique,
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const TEST_DIMS: usize = 8;

    fn sample_doc() -> Doc {
        Doc {
            id: 0,
            title: "Test Doc".to_string(),
            source_type: Some("md".to_string()),
            sha256: "deadbeef".repeat(8),
            source_path: None,
            source_size: Some(1234),
            source_mtime: Some("2026-07-18T00:00:00Z".to_string()),
            extraction_quality: None,
            added_at: "2026-07-18T00:00:00Z".to_string(),
        }
    }

    fn sample_chunk(doc_id: i64) -> Chunk {
        Chunk {
            id: 0,
            doc_id,
            section_path: "Intro > Setup".to_string(),
            locator: "test.md#L1-L4".to_string(),
            prefix: "Intro > Setup".to_string(),
            text: "The quick brown fox jumps over the lazy dog".to_string(),
            token_count: 10,
        }
    }

    // 1. Create a fresh `.kpack`, assert all five tables/vtables exist.
    #[test]
    fn t1_fresh_pack_has_all_five_tables() {
        let dir = unique_dir("t1");
        let path = dir.join("t1.kpack");
        let pack = Pack::open_or_create(&path, TEST_DIMS).unwrap();

        for name in ["docs", "chunks", "vec", "fts", "manifest"] {
            let exists: bool = pack
                .conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = ?1)",
                    params![name],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(exists, "table/vtable {name} should exist on a fresh pack");
        }
    }

    // 2. Insert a doc + a chunk (non-null section_path + locator), read them
    //    back equal.
    #[test]
    fn t2_insert_and_read_back_doc_and_chunk() {
        let dir = unique_dir("t2");
        let path = dir.join("t2.kpack");
        let pack = Pack::open_or_create(&path, TEST_DIMS).unwrap();

        let doc = sample_doc();
        let doc_id = pack.insert_doc(&doc).unwrap();
        let got_doc = pack.get_doc(doc_id).unwrap().unwrap();
        assert_eq!(got_doc.title, "Test Doc");
        assert_eq!(got_doc.sha256, doc.sha256);
        assert_eq!(got_doc.added_at, doc.added_at);
        assert_eq!(got_doc.source_type, doc.source_type);
        assert_eq!(got_doc.source_path, None);
        assert_eq!(got_doc.id, doc_id);

        let chunk = sample_chunk(doc_id);
        let chunk_id = pack.insert_chunk(&chunk).unwrap();
        let got_chunk = pack.get_chunk(chunk_id).unwrap().unwrap();
        assert_eq!(got_chunk.doc_id, doc_id);
        assert_eq!(got_chunk.section_path, chunk.section_path);
        assert_eq!(got_chunk.locator, chunk.locator);
        assert_eq!(got_chunk.text, chunk.text);
        assert_eq!(got_chunk.token_count, chunk.token_count);
        assert!(!got_chunk.section_path.is_empty());
        assert!(!got_chunk.locator.is_empty());
    }

    // 2b. A root chunk (no heading ancestry) uses an empty section_path —
    // never NULL (D6) — and that's a valid, distinct insert.
    #[test]
    fn t2b_root_chunk_uses_empty_section_path_not_null() {
        let dir = unique_dir("t2b");
        let path = dir.join("t2b.kpack");
        let pack = Pack::open_or_create(&path, TEST_DIMS).unwrap();
        let doc_id = pack.insert_doc(&sample_doc()).unwrap();

        let root_chunk = Chunk {
            id: 0,
            doc_id,
            section_path: String::new(),
            locator: "test.md#L1".to_string(),
            prefix: String::new(),
            text: "root-level text".to_string(),
            token_count: 3,
        };
        let chunk_id = pack.insert_chunk(&root_chunk).unwrap();
        let got = pack.get_chunk(chunk_id).unwrap().unwrap();
        assert_eq!(got.section_path, "");
    }

    // 3. Insert an int8 embedding, `vec_search` a near query vector →
    //    returns that chunk_id (the vec0 proof).
    #[test]
    fn t3_vec_search_finds_nearby_embedding() {
        let dir = unique_dir("t3");
        let path = dir.join("t3.kpack");
        let pack = Pack::open_or_create(&path, TEST_DIMS).unwrap();
        let doc_id = pack.insert_doc(&sample_doc()).unwrap();
        let chunk_id = pack.insert_chunk(&sample_chunk(doc_id)).unwrap();

        let vector: [i8; TEST_DIMS] = [10, -20, 30, -40, 50, -60, 70, -80];
        pack.insert_embedding(chunk_id, &vector).unwrap();

        // A second, unrelated chunk+embedding far away in vector space, so
        // this also proves ranking, not just "any row comes back".
        let other_chunk_id = pack.insert_chunk(&sample_chunk(doc_id)).unwrap();
        let far_vector: [i8; TEST_DIMS] = [-100, 100, -100, 100, -100, 100, -100, 100];
        pack.insert_embedding(other_chunk_id, &far_vector).unwrap();

        let near_query: [i8; TEST_DIMS] = [11, -19, 29, -41, 49, -59, 71, -79];
        let results = pack.vec_search(&near_query, 1).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].0, chunk_id,
            "nearest neighbor should be the close vector's chunk"
        );
    }

    // 4. `fts_search` for a word in the chunk text → returns that chunk_id
    //    (the fts5 proof).
    #[test]
    fn t4_fts_search_finds_matching_chunk() {
        let dir = unique_dir("t4");
        let path = dir.join("t4.kpack");
        let pack = Pack::open_or_create(&path, TEST_DIMS).unwrap();
        let doc_id = pack.insert_doc(&sample_doc()).unwrap();
        let chunk_id = pack.insert_chunk(&sample_chunk(doc_id)).unwrap();

        let results = pack.fts_search("brown", 5).unwrap();
        assert_eq!(results, vec![chunk_id]);
    }

    // 5. `manifest_set`/`manifest_get` round-trip; `schema_version` present
    //    == SCHEMA_VERSION.
    #[test]
    fn t5_manifest_roundtrip_and_schema_version() {
        let dir = unique_dir("t5");
        let path = dir.join("t5.kpack");
        let pack = Pack::open_or_create(&path, TEST_DIMS).unwrap();

        pack.manifest_set("pack_title", "Demo Pack").unwrap();
        assert_eq!(
            pack.manifest_get("pack_title").unwrap(),
            Some("Demo Pack".to_string())
        );
        assert_eq!(pack.manifest_get("missing_key").unwrap(), None);

        let version = pack.manifest_get("schema_version").unwrap().unwrap();
        assert_eq!(version, SCHEMA_VERSION.to_string());
    }

    // 6. Reopen the file with `open()` → data still present (persistence).
    #[test]
    fn t6_reopen_persists_data() {
        let dir = unique_dir("t6");
        let path = dir.join("t6.kpack");
        let doc_id;
        let chunk_id;
        {
            let pack = Pack::open_or_create(&path, TEST_DIMS).unwrap();
            doc_id = pack.insert_doc(&sample_doc()).unwrap();
            chunk_id = pack.insert_chunk(&sample_chunk(doc_id)).unwrap();
            let vector: [i8; TEST_DIMS] = [1, 2, 3, 4, 5, 6, 7, 8];
            pack.insert_embedding(chunk_id, &vector).unwrap();
            pack.manifest_set("k", "v").unwrap();
        }

        let reopened = Pack::open(&path).unwrap();
        assert_eq!(reopened.get_doc(doc_id).unwrap().unwrap().id, doc_id);
        assert_eq!(
            reopened.get_chunk(chunk_id).unwrap().unwrap().id,
            chunk_id
        );
        assert_eq!(reopened.manifest_get("k").unwrap(), Some("v".to_string()));
        assert_eq!(
            reopened.manifest_get("schema_version").unwrap(),
            Some(SCHEMA_VERSION.to_string())
        );
        let hits = reopened.vec_search(&[1, 2, 3, 4, 5, 6, 7, 8], 1).unwrap();
        assert_eq!(hits[0].0, chunk_id);
    }

    // open() on a non-.kpack (fresh empty SQLite file with no schema) fails
    // with a clear Schema error rather than panicking or silently succeeding.
    #[test]
    fn open_rejects_file_without_schema() {
        let dir = unique_dir("open-rejects");
        let path = dir.join("empty.kpack");
        {
            // Create an empty (schema-less) SQLite file directly.
            Connection::open(&path).unwrap();
        }
        let result = Pack::open(&path);
        assert!(matches!(result, Err(Error::Schema(_))));
    }

    // 7. `insert_embedding` with a vector length that doesn't match the
    //    pack's dims fails with a clear Schema error, not an opaque vec0
    //    rejection (Fix 5).
    #[test]
    fn t7_insert_embedding_rejects_dims_mismatch() {
        let dir = unique_dir("t7");
        let path = dir.join("t7.kpack");
        let pack = Pack::open_or_create(&path, TEST_DIMS).unwrap();
        let doc_id = pack.insert_doc(&sample_doc()).unwrap();
        let chunk_id = pack.insert_chunk(&sample_chunk(doc_id)).unwrap();

        let wrong_len_vector: [i8; 3] = [1, 2, 3];
        let result = pack.insert_embedding(chunk_id, &wrong_len_vector);
        assert!(matches!(result, Err(Error::Schema(_))));
    }

    // 8. Golden-DDL test: the exact ordered column name+type set for `docs`
    //    and `chunks` matches spec §1.1 — the lock that would have caught
    //    the original schema gap.
    #[test]
    fn t8_golden_ddl_matches_spec() {
        let dir = unique_dir("t8");
        let path = dir.join("t8.kpack");
        let pack = Pack::open_or_create(&path, TEST_DIMS).unwrap();

        let docs_cols = table_info(&pack.conn, "docs");
        assert_eq!(
            docs_cols,
            vec![
                ("id".to_string(), "INTEGER".to_string()),
                ("title".to_string(), "TEXT".to_string()),
                ("source_type".to_string(), "TEXT".to_string()),
                ("sha256".to_string(), "TEXT".to_string()),
                ("source_path".to_string(), "TEXT".to_string()),
                ("source_size".to_string(), "INTEGER".to_string()),
                ("source_mtime".to_string(), "TEXT".to_string()),
                ("extraction_quality".to_string(), "REAL".to_string()),
                ("added_at".to_string(), "TEXT".to_string()),
            ]
        );

        let chunks_cols = table_info(&pack.conn, "chunks");
        assert_eq!(
            chunks_cols,
            vec![
                ("id".to_string(), "INTEGER".to_string()),
                ("doc_id".to_string(), "INTEGER".to_string()),
                ("section_path".to_string(), "TEXT".to_string()),
                ("locator".to_string(), "TEXT".to_string()),
                ("prefix".to_string(), "TEXT".to_string()),
                ("text".to_string(), "TEXT".to_string()),
                ("token_count".to_string(), "INTEGER".to_string()),
            ]
        );
    }

    /// `PRAGMA table_info(name)` as an ordered `(column_name, declared_type)`
    /// list, in table-definition order (the pragma's `cid` order).
    fn table_info(conn: &Connection, table: &str) -> Vec<(String, String)> {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap();
        stmt.query_map([], |row| {
            let name: String = row.get(1)?;
            let ty: String = row.get(2)?;
            Ok((name, ty))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap()
    }

    // 9. Foreign key enforcement: a chunk referencing a missing doc_id fails
    //    loudly (Fix 4) now that PRAGMA foreign_keys = ON is set per
    //    connection.
    #[test]
    fn t9_foreign_key_enforced_on_missing_doc() {
        let dir = unique_dir("t9");
        let path = dir.join("t9.kpack");
        let pack = Pack::open_or_create(&path, TEST_DIMS).unwrap();

        let orphan_chunk = sample_chunk(999_999);
        let result = pack.insert_chunk(&orphan_chunk);
        assert!(
            result.is_err(),
            "inserting a chunk with a non-existent doc_id should fail with FK enforcement on"
        );
    }
}
