//! Local SQLite conversation store (§7 S7-1): folders/chats/messages plus an
//! `fts5` table for search over chat titles + message content. This is the
//! FOUNDATION of §7 (a ChatGPT/Claude-style sidebar) — before this module
//! the app has no conversation store at all; chat is ephemeral in `src/app.js`'s
//! `state.chat` and is lost on exit. Backend-only this slice: the store +
//! the full CRUD command surface, no front-end wiring (S7-2 builds the
//! sidebar UI against these commands; S7-3 wires chat send/generate to
//! actually call them). Design principle (§7): the model stays stateless
//! and never knows chats exist — this is entirely app-side, in one local
//! file under the app's data dir.
//!
//! Privacy (§3.5): the store is LOCAL-ONLY. It is never included in
//! telemetry, logs, or any cloud/network path — everything in this module
//! is plain synchronous SQLite I/O against a file on disk (or `:memory:`
//! for tests), nothing here ever opens a socket.
//!
//! ## Command style
//! Mirrors `kpack.rs`: thin `#[tauri::command] async fn` -> `spawn_blocking`
//! around a pure [`ConvStore`] method, camelCase serde DTOs (Tauri's command
//! macro already maps a JS `camelCase` argument to the matching Rust
//! `snake_case` parameter, so command *parameters* need no `#[serde(rename)]`
//! — only the DTOs returned across IPC do). `ConvStore` is managed directly
//! (`app.manage(ConvStore::open(..)?)` in `main.rs`, not wrapped in an outer
//! `Arc`) — same as `kpack.rs`'s `EmbedderCache`/`Builds` — so a command
//! moves its `AppHandle` into the `spawn_blocking` closure and re-fetches
//! `app.state::<ConvStore>()` there rather than cloning an `Arc` up front.
//! `ConvStore` itself is `Send + Sync` (a `Mutex<Connection>`; SQLite is
//! single-writer per connection anyway, so one mutex-guarded connection for
//! the whole app is the right amount of concurrency here — this is a
//! desktop app, not a server).
//!
//! ## `chat_fts` sync (§7.1/§7.5)
//! `chat_fts` is a standalone `fts5(title, content)` table, NOT the
//! `content=''` external-content mode `format.rs`'s pack-search `fts` table
//! uses (that's a nicety here, not a requirement — see the task brief).
//! It holds exactly one row per chat, with an EXPLICIT `rowid` set to that
//! chat's `id` (fts5 tables accept an explicit rowid on `INSERT` like any
//! ordinary table) — that rowid is the join key back to `chats.id`, since
//! the fts5 schema itself (per the brief) carries no separate id column.
//! Kept in sync by this module's write paths directly (no triggers):
//! - `create_chat` inserts the row (`title` = the chat's title, `content` =
//!   `''`).
//! - `rename_chat` updates just the `title` column for that rowid.
//! - `append_message` appends the new message's content onto the existing
//!   `content` column for that rowid (`content = content || ' ' || ?`) —
//!   fts5 tables support ordinary `UPDATE`/`INSERT`/`DELETE` like a normal
//!   table (SQLite's own fts5 documentation), including a partial-column
//!   `SET` that references the column's own old value.
//! - `delete_chat` issues `DELETE FROM chat_fts WHERE rowid = ?` for that
//!   chat, in addition to `DELETE FROM chats` (whose `messages` cascade is
//!   via `ON DELETE CASCADE` + `PRAGMA foreign_keys = ON`) — so a deleted
//!   chat leaves NO orphan fts row (§7.5's deletion invariant, locked down
//!   by `t4_delete_chat_leaves_no_orphan_fts_row` below).
//!
//! `search_chats` guards fts5 special characters the same way the pack
//! search does — reusing `kpack_core::retrieve::safe_fts5_query` (already a
//! transitive dependency via `kpack-core`) rather than re-implementing the
//! same quoting logic here.
//!
//! ## Inference provenance (forward-compat for the future LoRA-adapter track)
//! `chats.model_id`/`chats.adapter_ids` record which model + adapters were
//! active when a chat was created, the same way `mounted_packs` already
//! records which knowledge packs were active — so that when the
//! adapter-swapping track lands, opening an old chat can restore its
//! specialist exactly as it already remounts packs, with zero future schema
//! change. `model_id` is a constant hero-model value today (there's only
//! one model); `adapter_ids` is `[]` today (no adapters exist yet) — both
//! stamped once at `create_chat` time and otherwise immutable (unlike
//! `mounted_packs`, there's no `set_chat_model`/`set_chat_adapters`: v1 has
//! nothing for such a setter to change). Deliberately chat-level, not
//! per-message: per-message citations already give per-message PACK
//! provenance, but per-message model/adapter provenance is a possible
//! future refinement, not needed yet.
//!
//! A dev database created before these two columns existed gets them via a
//! tiny defensive migration in [`ConvStore::from_connection`]
//! (`PRAGMA table_info(chats)` + `ALTER TABLE ... ADD COLUMN` for whichever
//! is missing) — `CREATE TABLE IF NOT EXISTS` alone is a no-op against an
//! already-existing `chats` table of the OLD shape, so without this an
//! existing dev DB would error on the next `INSERT`/`SELECT` that touches
//! either column.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;

/// Schema (mirrors spec §7.1 exactly). `PRAGMA foreign_keys = ON` is set
/// here too, once, at connection-open time — it's a per-connection setting
/// in SQLite, and this store holds exactly one connection for its whole
/// lifetime, so there's nowhere else it needs to be re-set.
const SCHEMA_SQL: &str = "
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS folders (id INTEGER PRIMARY KEY, name TEXT NOT NULL, sort_order INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS chats (
  id INTEGER PRIMARY KEY, folder_id INTEGER REFERENCES folders(id),
  title TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
  pinned INTEGER NOT NULL DEFAULT 0, archived INTEGER NOT NULL DEFAULT 0,
  mounted_packs TEXT,
  model_id TEXT NOT NULL DEFAULT '',
  adapter_ids TEXT NOT NULL DEFAULT '[]'
);
CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY, chat_id INTEGER NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
  role TEXT NOT NULL, content TEXT NOT NULL,
  citations TEXT,
  tool_calls TEXT,
  created_at TEXT NOT NULL
);
CREATE VIRTUAL TABLE IF NOT EXISTS chat_fts USING fts5(title, content);
";

/// Column list shared by every `chats` read query, so the positional
/// `chat_from_row` mapping below stays correct no matter which query built
/// the row.
const CHAT_COLUMNS: &str = "id, folder_id, title, created_at, updated_at, pinned, archived, \
     mounted_packs, model_id, adapter_ids";

/// Column list shared by every `messages` read query — same rationale as
/// `CHAT_COLUMNS`.
const MESSAGE_COLUMNS: &str = "id, role, content, citations, tool_calls, created_at";

/// The local conversation store: one mutex-guarded SQLite connection.
/// `open`/`open_in_memory` both run schema init before returning, so every
/// `ConvStore` in hand is immediately ready to use — callers never need a
/// separate "migrate" step.
pub struct ConvStore {
    conn: Mutex<Connection>,
}

impl ConvStore {
    /// Opens (or creates) the on-disk database at `path` and runs schema
    /// init. `main.rs` calls this with `<app_data_dir>/conversations.db`.
    pub fn open(path: &Path) -> Result<ConvStore, String> {
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        Self::from_connection(conn)
    }

    /// An in-memory store (`:memory:`) for tests — exercises the exact same
    /// schema/CRUD code as `open`, just without touching disk. `#[cfg(test)]`
    /// only: `cleophis` is a binary crate with no other crate depending on
    /// it, so (unlike `kpack-core`'s cross-crate `MockEmbedder`, which needs
    /// a `test-util` Cargo feature to stay visible to a DIFFERENT crate's
    /// tests) a plain `#[cfg(test)]` is enough to keep this reachable from
    /// this module's own `#[cfg(test)] mod tests` below without leaving it
    /// as dead code in a normal (non-test) build.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<ConvStore, String> {
        let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> Result<ConvStore, String> {
        conn.execute_batch(SCHEMA_SQL).map_err(|e| e.to_string())?;
        Self::migrate_chats_columns(&conn)?;
        Ok(ConvStore {
            conn: Mutex::new(conn),
        })
    }

    /// Defensive migration for a dev `conversations.db` created before
    /// `chats.model_id`/`chats.adapter_ids` existed — see the module doc
    /// comment's "Inference provenance" section. `CREATE TABLE IF NOT
    /// EXISTS` in `SCHEMA_SQL` is a no-op against an already-existing
    /// `chats` table regardless of its column shape, so this checks
    /// `PRAGMA table_info(chats)` for each of the two columns and
    /// `ALTER TABLE ... ADD COLUMN`s whichever is missing. Both defaults
    /// are plain string literals (`''`/`'[]'`), which SQLite allows on a
    /// `NOT NULL ADD COLUMN` (it backfills every existing row with that
    /// literal) — only a non-constant default like `CURRENT_TIMESTAMP`
    /// would be rejected there. A brand-new database (via `SCHEMA_SQL`'s
    /// `CREATE TABLE`) already has both columns, so this is a no-op for it.
    fn migrate_chats_columns(conn: &Connection) -> Result<(), String> {
        let mut existing = std::collections::HashSet::new();
        {
            let mut stmt = conn
                .prepare("PRAGMA table_info(chats)")
                .map_err(|e| e.to_string())?;
            let names = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .map_err(|e| e.to_string())?;
            for name in names {
                existing.insert(name.map_err(|e| e.to_string())?);
            }
        }
        if !existing.contains("model_id") {
            conn.execute(
                "ALTER TABLE chats ADD COLUMN model_id TEXT NOT NULL DEFAULT ''",
                [],
            )
            .map_err(|e| e.to_string())?;
        }
        if !existing.contains("adapter_ids") {
            conn.execute(
                "ALTER TABLE chats ADD COLUMN adapter_ids TEXT NOT NULL DEFAULT '[]'",
                [],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    // ---- Folders ----------------------------------------------------

    pub fn create_folder(&self, name: &str) -> Result<FolderInfo, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO folders (name, sort_order) VALUES (?1, 0)",
            params![name],
        )
        .map_err(|e| e.to_string())?;
        Ok(FolderInfo {
            id: conn.last_insert_rowid(),
            name: name.to_string(),
            sort_order: 0,
        })
    }

    /// By `sort_order` then `id` (spec §7.1) — the sidebar's folder order.
    pub fn list_folders(&self) -> Result<Vec<FolderInfo>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, name, sort_order FROM folders ORDER BY sort_order, id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(FolderInfo {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    sort_order: row.get(2)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    pub fn rename_folder(&self, id: i64, name: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE folders SET name = ?1 WHERE id = ?2",
            params![name, id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// UNFILES the folder's chats (`folder_id` -> `NULL`) before deleting
    /// the folder itself — the chats survive, per spec §7.1's "delete_folder
    /// unfiles, does not delete" rule.
    pub fn delete_folder(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE chats SET folder_id = NULL WHERE folder_id = ?1",
            params![id],
        )
        .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM folders WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ---- Chats --------------------------------------------------------

    /// `model_id`/`adapter_ids` are the inference provenance stamped once at
    /// creation (see the module doc comment's "Inference provenance"
    /// section) — the FE passes the currently-active model/adapters, the
    /// same way it passes the currently-active `mounted_packs`.
    pub fn create_chat(
        &self,
        title: &str,
        folder_id: Option<i64>,
        mounted_packs: Option<Vec<String>>,
        model_id: &str,
        adapter_ids: Vec<String>,
    ) -> Result<ChatInfo, String> {
        let conn = self.conn.lock().unwrap();
        let now = now_iso();
        let mounted_packs = mounted_packs.unwrap_or_default();
        let packs_json = serde_json::to_string(&mounted_packs).map_err(|e| e.to_string())?;
        let adapter_ids_json = serde_json::to_string(&adapter_ids).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO chats (folder_id, title, created_at, updated_at, pinned, archived, mounted_packs, model_id, adapter_ids)
             VALUES (?1, ?2, ?3, ?3, 0, 0, ?4, ?5, ?6)",
            params![folder_id, title, now, packs_json, model_id, adapter_ids_json],
        )
        .map_err(|e| e.to_string())?;
        let id = conn.last_insert_rowid();

        // fts sync: the chat's title row (content starts empty — see the
        // module doc comment).
        conn.execute(
            "INSERT INTO chat_fts(rowid, title, content) VALUES (?1, ?2, '')",
            params![id, title],
        )
        .map_err(|e| e.to_string())?;

        Ok(ChatInfo {
            id,
            folder_id,
            title: title.to_string(),
            created_at: now.clone(),
            updated_at: now,
            pinned: false,
            archived: false,
            mounted_packs,
            model_id: model_id.to_string(),
            adapter_ids,
        })
    }

    /// Pinned-first, then most-recently-updated (spec §7.1) — the sidebar's
    /// chat order.
    pub fn list_chats(&self) -> Result<Vec<ChatInfo>, String> {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT {CHAT_COLUMNS} FROM chats ORDER BY pinned DESC, updated_at DESC, id DESC"
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], chat_from_row)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    /// The chat plus its messages, ordered `created_at` then `id` ascending
    /// (conversation order).
    pub fn get_chat(&self, id: i64) -> Result<ChatDetail, String> {
        let conn = self.conn.lock().unwrap();
        let sql = format!("SELECT {CHAT_COLUMNS} FROM chats WHERE id = ?1");
        let chat = conn
            .query_row(&sql, params![id], chat_from_row)
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => "no such chat".to_string(),
                other => other.to_string(),
            })?;

        let msg_sql =
            format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE chat_id = ?1 ORDER BY created_at ASC, id ASC");
        let mut stmt = conn.prepare(&msg_sql).map_err(|e| e.to_string())?;
        let messages = stmt
            .query_map(params![id], message_from_row)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;

        Ok(ChatDetail { chat, messages })
    }

    /// Bumps `updated_at` (spec §7.4) and updates the fts title row.
    pub fn rename_chat(&self, id: i64, title: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let now = now_iso();
        conn.execute(
            "UPDATE chats SET title = ?1, updated_at = ?2 WHERE id = ?3",
            params![title, now, id],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE chat_fts SET title = ?1 WHERE rowid = ?2",
            params![title, id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Real delete: `messages` cascade via `ON DELETE CASCADE` (`PRAGMA
    /// foreign_keys = ON`, set once at connection-open time), and the
    /// chat's `chat_fts` row is removed explicitly — see the module doc
    /// comment for why `chat_fts` needs its own `DELETE` (it's not wired to
    /// `chats` by a foreign key; SQLite has no cascade for virtual tables).
    pub fn delete_chat(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM chats WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM chat_fts WHERE rowid = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn set_chat_pinned(&self, id: i64, pinned: bool) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE chats SET pinned = ?1 WHERE id = ?2",
            params![pinned, id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn set_chat_archived(&self, id: i64, archived: bool) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE chats SET archived = ?1 WHERE id = ?2",
            params![archived, id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn move_chat(&self, id: i64, folder_id: Option<i64>) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE chats SET folder_id = ?1 WHERE id = ?2",
            params![folder_id, id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn set_chat_packs(&self, id: i64, mounted_packs: Vec<String>) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let packs_json = serde_json::to_string(&mounted_packs).map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE chats SET mounted_packs = ?1 WHERE id = ?2",
            params![packs_json, id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ---- Messages -------------------------------------------------------

    /// Inserts the message, bumps the parent chat's `updated_at` (spec
    /// §7.4), and syncs `chat_fts` (see the module doc comment).
    pub fn append_message(
        &self,
        chat_id: i64,
        role: &str,
        content: &str,
        citations: Option<Value>,
        tool_calls: Option<Value>,
    ) -> Result<MessageInfo, String> {
        let conn = self.conn.lock().unwrap();
        let now = now_iso();
        let citations_json = citations.as_ref().map(|v| v.to_string());
        let tool_calls_json = tool_calls.as_ref().map(|v| v.to_string());

        conn.execute(
            "INSERT INTO messages (chat_id, role, content, citations, tool_calls, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![chat_id, role, content, citations_json, tool_calls_json, now],
        )
        .map_err(|e| e.to_string())?;
        let id = conn.last_insert_rowid();

        conn.execute(
            "UPDATE chats SET updated_at = ?1 WHERE id = ?2",
            params![now, chat_id],
        )
        .map_err(|e| e.to_string())?;

        conn.execute(
            "UPDATE chat_fts SET content = content || ' ' || ?1 WHERE rowid = ?2",
            params![content, chat_id],
        )
        .map_err(|e| e.to_string())?;

        Ok(MessageInfo {
            id,
            role: role.to_string(),
            content: content.to_string(),
            citations,
            tool_calls,
            created_at: now,
        })
    }

    // ---- Search -----------------------------------------------------

    /// Chats whose title OR any message content matches `query`, via
    /// `chat_fts` (pinned-first / most-recently-updated, same order as
    /// `list_chats` — a search result list is still a chat list). `query`
    /// is run through `safe_fts5_query` (guards fts5 special characters the
    /// same way the pack search does); an empty/whitespace `query`
    /// (`safe_fts5_query` returns `""` for one) short-circuits to no
    /// results rather than reaching `MATCH` with an empty argument (which
    /// fts5 treats as a syntax error) — an empty search box showing nothing
    /// is the more intuitive UX than showing every chat.
    pub fn search_chats(&self, query: &str) -> Result<Vec<ChatInfo>, String> {
        let safe_query = kpack_core::retrieve::safe_fts5_query(query);
        if safe_query.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT {CHAT_COLUMNS} FROM chats
             WHERE id IN (SELECT rowid FROM chat_fts WHERE chat_fts MATCH ?1)
             ORDER BY pinned DESC, updated_at DESC, id DESC"
        );
        let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![safe_query], chat_from_row)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }
}

/// ISO-8601 local timestamp with a numeric `+TZ` offset (spec §7.4) — e.g.
/// `2026-07-19T14:23:01.123456789-07:00`. Nanosecond fractional precision
/// (`SecondsFormat::Nanos`) is deliberate, not decorative: it's what keeps
/// two `now_iso()` calls a few microseconds apart (as `append_message`
/// makes on every call) distinguishable even on platforms where the
/// underlying clock's reporting granularity is coarser than a second would
/// otherwise show.
fn now_iso() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, false)
}

fn chat_from_row(row: &rusqlite::Row) -> rusqlite::Result<ChatInfo> {
    let mounted_packs_json: Option<String> = row.get(7)?;
    let mounted_packs = mounted_packs_json
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default();
    let adapter_ids_json: String = row.get(9)?;
    let adapter_ids = serde_json::from_str::<Vec<String>>(&adapter_ids_json).unwrap_or_default();
    Ok(ChatInfo {
        id: row.get(0)?,
        folder_id: row.get(1)?,
        title: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
        pinned: row.get(5)?,
        archived: row.get(6)?,
        mounted_packs,
        model_id: row.get(8)?,
        adapter_ids,
    })
}

fn message_from_row(row: &rusqlite::Row) -> rusqlite::Result<MessageInfo> {
    let citations_json: Option<String> = row.get(3)?;
    let tool_calls_json: Option<String> = row.get(4)?;
    Ok(MessageInfo {
        id: row.get(0)?,
        role: row.get(1)?,
        content: row.get(2)?,
        citations: citations_json.and_then(|s| serde_json::from_str(&s).ok()),
        tool_calls: tool_calls_json.and_then(|s| serde_json::from_str(&s).ok()),
        created_at: row.get(5)?,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderInfo {
    pub id: i64,
    pub name: String,
    pub sort_order: i64,
}

/// A camelCase, front-end-facing view of a `chats` row — `mounted_packs`
/// round-trips through the column's JSON-array-of-paths encoding (spec
/// §7.1's comment on the `chats.mounted_packs` column) to a plain
/// `Vec<String>` here, and `adapter_ids` round-trips the same way from its
/// JSON-array-of-ids column. `model_id`/`adapter_ids` are the chat's
/// inference provenance (see the module doc comment's "Inference
/// provenance" section) — constant (`"hero-llama"`-style value / `[]`)
/// today, stamped once at `create_chat` time.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatInfo {
    pub id: i64,
    pub folder_id: Option<i64>,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub pinned: bool,
    pub archived: bool,
    pub mounted_packs: Vec<String>,
    pub model_id: String,
    pub adapter_ids: Vec<String>,
}

/// A camelCase, front-end-facing view of a `messages` row — `citations`/
/// `tool_calls` round-trip through the columns' JSON-text encoding to plain
/// `serde_json::Value`s here (`None` when the column is `NULL`, e.g. every
/// message today has no `tool_calls` — spec §7.1 notes it's unused so far).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageInfo {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub citations: Option<Value>,
    pub tool_calls: Option<Value>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatDetail {
    pub chat: ChatInfo,
    pub messages: Vec<MessageInfo>,
}

// ==== Tauri commands =====================================================
//
// Thin wrappers only: each clones nothing but the `AppHandle` it's handed,
// runs a pure `ConvStore` method on a `spawn_blocking` thread (SQLite I/O is
// blocking), and maps a join failure to the same user-safe message
// `kpack.rs`/`cloud::commands` already use. `ConvStore`'s own methods do all
// the real work and are what the tests below exercise directly.

use tauri::{AppHandle, Manager};

/// Only reachable if the blocking task itself panics or the runtime is
/// shutting down — mirrors `kpack.rs::JOIN_ERROR_MESSAGE`.
const JOIN_ERROR_MESSAGE: &str = "Something went wrong on this device. Please try again.";

#[tauri::command]
pub async fn create_folder(name: String, app: AppHandle) -> Result<FolderInfo, String> {
    tauri::async_runtime::spawn_blocking(move || app.state::<ConvStore>().create_folder(&name))
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn list_folders(app: AppHandle) -> Result<Vec<FolderInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || app.state::<ConvStore>().list_folders())
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn rename_folder(id: i64, name: String, app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ConvStore>().rename_folder(id, &name)
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn delete_folder(id: i64, app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || app.state::<ConvStore>().delete_folder(id))
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn create_chat(
    title: String,
    folder_id: Option<i64>,
    mounted_packs: Option<Vec<String>>,
    model_id: String,
    adapter_ids: Vec<String>,
    app: AppHandle,
) -> Result<ChatInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ConvStore>()
            .create_chat(&title, folder_id, mounted_packs, &model_id, adapter_ids)
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn list_chats(app: AppHandle) -> Result<Vec<ChatInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || app.state::<ConvStore>().list_chats())
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn get_chat(id: i64, app: AppHandle) -> Result<ChatDetail, String> {
    tauri::async_runtime::spawn_blocking(move || app.state::<ConvStore>().get_chat(id))
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn rename_chat(id: i64, title: String, app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || app.state::<ConvStore>().rename_chat(id, &title))
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn delete_chat(id: i64, app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || app.state::<ConvStore>().delete_chat(id))
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn set_chat_pinned(id: i64, pinned: bool, app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ConvStore>().set_chat_pinned(id, pinned)
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn set_chat_archived(id: i64, archived: bool, app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ConvStore>().set_chat_archived(id, archived)
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn move_chat(id: i64, folder_id: Option<i64>, app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || app.state::<ConvStore>().move_chat(id, folder_id))
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn set_chat_packs(
    id: i64,
    mounted_packs: Vec<String>,
    app: AppHandle,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ConvStore>().set_chat_packs(id, mounted_packs)
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn append_message(
    chat_id: i64,
    role: String,
    content: String,
    citations: Option<Value>,
    tool_calls: Option<Value>,
    app: AppHandle,
) -> Result<MessageInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ConvStore>()
            .append_message(chat_id, &role, &content, citations, tool_calls)
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn search_chats(query: String, app: AppHandle) -> Result<Vec<ChatInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || app.state::<ConvStore>().search_chats(&query))
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// create_chat -> append 2 messages (one with citations) -> get_chat
    /// returns the chat + both messages in conversation order, citations
    /// round-trip intact.
    #[test]
    fn t1_get_chat_returns_chat_and_messages_in_order_with_citations() {
        let store = ConvStore::open_in_memory().unwrap();
        let chat = store
            .create_chat("My chat", None, None, "hero-llama", vec![])
            .unwrap();

        let m1 = store
            .append_message(chat.id, "user", "hello", None, None)
            .unwrap();
        let citations = json!([{"n": 1, "docTitle": "Doc"}]);
        let m2 = store
            .append_message(chat.id, "assistant", "hi there", Some(citations.clone()), None)
            .unwrap();

        let detail = store.get_chat(chat.id).unwrap();
        assert_eq!(detail.chat.id, chat.id);
        assert_eq!(detail.messages.len(), 2);
        assert_eq!(detail.messages[0].id, m1.id);
        assert_eq!(detail.messages[0].content, "hello");
        assert!(detail.messages[0].citations.is_none());
        assert_eq!(detail.messages[1].id, m2.id);
        assert_eq!(detail.messages[1].content, "hi there");
        assert_eq!(detail.messages[1].citations, Some(citations));
    }

    /// list_chats ordering: pinned sorts before a newer unpinned chat;
    /// archived is reflected on the returned `ChatInfo`.
    #[test]
    fn t2_list_chats_orders_pinned_first_and_reflects_archived() {
        let store = ConvStore::open_in_memory().unwrap();
        let older = store
            .create_chat("Older", None, None, "hero-llama", vec![])
            .unwrap();
        let newer = store
            .create_chat("Newer", None, None, "hero-llama", vec![])
            .unwrap();
        store.set_chat_pinned(older.id, true).unwrap();
        store.set_chat_archived(newer.id, true).unwrap();

        let chats = store.list_chats().unwrap();
        assert_eq!(chats[0].id, older.id, "the pinned chat must sort first");
        assert!(chats[0].pinned);
        let newer_entry = chats.iter().find(|c| c.id == newer.id).unwrap();
        assert!(newer_entry.archived);
    }

    /// folders: create -> move a chat in -> list; delete_folder unfiles its
    /// chats (folder_id NULL) rather than deleting them.
    #[test]
    fn t3_delete_folder_unfiles_chats_instead_of_deleting_them() {
        let store = ConvStore::open_in_memory().unwrap();
        let folder = store.create_folder("Work").unwrap();
        let chat = store
            .create_chat("A chat", None, None, "hero-llama", vec![])
            .unwrap();
        store.move_chat(chat.id, Some(folder.id)).unwrap();

        let folders = store.list_folders().unwrap();
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].id, folder.id);
        let moved = store.get_chat(chat.id).unwrap().chat;
        assert_eq!(moved.folder_id, Some(folder.id));

        store.delete_folder(folder.id).unwrap();

        assert!(store.list_folders().unwrap().is_empty());
        let survived = store.get_chat(chat.id).unwrap().chat;
        assert_eq!(survived.folder_id, None, "the chat must survive, unfiled");
    }

    /// delete_chat: messages are gone (FK cascade) AND no fts rows survive
    /// — search for the deleted chat's content finds nothing (§7.5).
    #[test]
    fn t4_delete_chat_leaves_no_orphan_fts_row() {
        let store = ConvStore::open_in_memory().unwrap();
        let chat = store
            .create_chat("Doomed chat", None, None, "hero-llama", vec![])
            .unwrap();
        store
            .append_message(chat.id, "user", "unique_marker_xyz", None, None)
            .unwrap();

        assert_eq!(store.search_chats("unique_marker_xyz").unwrap().len(), 1);

        store.delete_chat(chat.id).unwrap();

        assert!(store.get_chat(chat.id).is_err(), "the chat itself is gone");
        assert!(
            store.search_chats("unique_marker_xyz").unwrap().is_empty(),
            "no orphan fts row should match the deleted chat's content"
        );
        assert!(
            store.search_chats("Doomed").unwrap().is_empty(),
            "no orphan fts row should match the deleted chat's title either"
        );
    }

    /// search_chats finds a chat by a word in its title and by a word in a
    /// message's content; a deleted chat never appears in results.
    #[test]
    fn t5_search_chats_matches_title_and_message_content_not_deleted_chats() {
        let store = ConvStore::open_in_memory().unwrap();
        let by_title = store
            .create_chat("Vitamin K research", None, None, "hero-llama", vec![])
            .unwrap();
        let by_content = store
            .create_chat("Untitled", None, None, "hero-llama", vec![])
            .unwrap();
        store
            .append_message(by_content.id, "user", "tell me about blood clotting", None, None)
            .unwrap();
        let gone = store
            .create_chat("clotting notes", None, None, "hero-llama", vec![])
            .unwrap();
        store.delete_chat(gone.id).unwrap();

        let title_hits = store.search_chats("Vitamin").unwrap();
        assert_eq!(title_hits.len(), 1);
        assert_eq!(title_hits[0].id, by_title.id);

        let content_hits = store.search_chats("clotting").unwrap();
        assert_eq!(content_hits.len(), 1);
        assert_eq!(content_hits[0].id, by_content.id);
    }

    /// timestamps: created_at/updated_at are non-empty ISO-8601, and
    /// updated_at advances on append. A short sleep between the two writes
    /// guards against coarse clock resolution on some platforms (Windows'
    /// default `SystemTime` granularity can be tens of ms) even though
    /// `now_iso()` already requests nanosecond fractional precision.
    #[test]
    fn t6_timestamps_are_iso8601_and_updated_at_advances_on_append() {
        let store = ConvStore::open_in_memory().unwrap();
        let chat = store
            .create_chat("Timing", None, None, "hero-llama", vec![])
            .unwrap();
        assert!(!chat.created_at.is_empty());
        assert!(!chat.updated_at.is_empty());
        assert_eq!(chat.created_at, chat.updated_at);

        std::thread::sleep(std::time::Duration::from_millis(20));
        store
            .append_message(chat.id, "user", "first", None, None)
            .unwrap();
        let after_first = store.get_chat(chat.id).unwrap().chat;
        assert!(after_first.updated_at > chat.updated_at);

        std::thread::sleep(std::time::Duration::from_millis(20));
        store
            .append_message(chat.id, "assistant", "second", None, None)
            .unwrap();
        let after_second = store.get_chat(chat.id).unwrap().chat;
        assert!(after_second.updated_at > after_first.updated_at);
    }

    /// create_chat's model_id/adapter_ids provenance round-trips through
    /// both get_chat and list_chats (forward-compat for the future
    /// LoRA-adapter track — see the module doc comment).
    #[test]
    fn t7_model_and_adapter_provenance_round_trips() {
        let store = ConvStore::open_in_memory().unwrap();
        let chat = store
            .create_chat(
                "Specialist chat",
                None,
                None,
                "hero-llama",
                vec!["med-adapter-v1".to_string()],
            )
            .unwrap();
        assert_eq!(chat.model_id, "hero-llama");
        assert_eq!(chat.adapter_ids, vec!["med-adapter-v1".to_string()]);

        let fetched = store.get_chat(chat.id).unwrap().chat;
        assert_eq!(fetched.model_id, "hero-llama");
        assert_eq!(fetched.adapter_ids, vec!["med-adapter-v1".to_string()]);

        let listed = store.list_chats().unwrap();
        let listed_chat = listed.iter().find(|c| c.id == chat.id).unwrap();
        assert_eq!(listed_chat.model_id, "hero-llama");
        assert_eq!(listed_chat.adapter_ids, vec!["med-adapter-v1".to_string()]);
    }

    /// Defensive migration (task amendment): a `chats` table created with
    /// the OLD (pre-model_id/adapter_ids) shape — simulated here by hand,
    /// bypassing `SCHEMA_SQL` — still opens cleanly through
    /// `ConvStore::from_connection`, and `create_chat` against it succeeds
    /// with the new columns backfilled to their defaults rather than
    /// erroring on the missing columns.
    #[test]
    fn t8_migrates_an_existing_chats_table_missing_the_new_columns() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE chats (
                id INTEGER PRIMARY KEY, folder_id INTEGER REFERENCES folders(id),
                title TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
                pinned INTEGER NOT NULL DEFAULT 0, archived INTEGER NOT NULL DEFAULT 0,
                mounted_packs TEXT
            );",
        )
        .unwrap();

        let store = ConvStore::from_connection(conn).unwrap();
        let chat = store
            .create_chat("Migrated", None, None, "hero-llama", vec![])
            .unwrap();
        assert_eq!(chat.model_id, "hero-llama");
        assert!(chat.adapter_ids.is_empty());
    }
}
