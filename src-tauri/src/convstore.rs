//! Local SQLite conversation store (§7 S7-1): folders/chats/messages plus an
//! `fts5` table for search over chat titles + message content. This is the
//! FOUNDATION of §7 (a ChatGPT/Claude-style sidebar) — before this module
//! the app has no conversation store at all; chat is ephemeral in `src/app.js`'s
//! `state.chat` and is lost on exit. Design principle (§7): the model stays
//! stateless and never knows chats exist — this is entirely app-side, in
//! local files under the app's data dir.
//!
//! Privacy (§3.5): the store is LOCAL-ONLY. It is never included in
//! telemetry, logs, or any cloud/network path — everything in this module
//! is plain synchronous SQLite I/O against files on disk (or `:memory:`
//! for tests), nothing here ever opens a socket.
//!
//! ## Per-account isolation (§7-chatiso — SECURITY)
//! `ConvStore` holds ONE SQLITE DATABASE PER ACCOUNT, not one shared
//! database — the same fix A6 already applied to knowledge packs
//! (`kpack.rs`'s `packs_dir`), applied here to conversations. Before this,
//! every account's chats lived in one device-global `conversations.db`, so
//! the sidebar showed EVERY signed-in account's chats to whoever was
//! currently signed in — a cross-account privacy leak (user-reported,
//! found bouncing between 3 accounts). A per-query `WHERE owner = ?` filter
//! would work too, but is a filter to forget on every future query; a
//! per-account database file is isolation BY CONSTRUCTION — there is no
//! query that can accidentally cross accounts, because another account's
//! rows are not in the file being queried at all, and `chat_fts` search is
//! scoped for free the same way.
//!
//! `ConvStore { root: StoreRoot, conns: Mutex<HashMap<String, Arc<Mutex<Connection>>>> }`:
//! `root` says where a fresh account's database comes from
//! (`StoreRoot::Dir(<app_data>/conversations/)` for the real app —
//! `<segment>.db` per account — or `StoreRoot::Memory` for tests, a fresh
//! `:memory:` connection per account); `conns` is a lazily-populated cache
//! keyed by the sanitized account segment, so an account's database is
//! opened (and schema-initialized/migrated) at most once per process.
//! [`account_dir_segment`] is a verbatim mirror of `kpack.rs`'s
//! module-private function of the same name (that one isn't reachable from
//! here, hence the mirror rather than a shared import) — same strict
//! `[A-Za-z0-9_-]`, non-empty, reject-don't-strip discipline, so a
//! malformed id can never collapse onto a shared path or escape `root` via
//! `..`/separators.
//!
//! EVERY [`ConvStore`] method takes `user_id: &str` as its first parameter
//! and resolves the per-account connection via [`ConvStore::conn_for`]
//! before doing anything else. Every `#[tauri::command]` below resolves
//! that `user_id` itself from the AUTHORITATIVE `Cloud` session state
//! (`current_user_id`, mirroring A6's `packs_dir`) — NEVER from a
//! front-end-supplied argument, so a compromised/buggy renderer can't name
//! a different account's id and read its chats. Signed out
//! (`current_user_id() == None`) never touches the store at all: reads
//! (`list_chats`/`list_folders`/`search_chats`) return an empty result,
//! `get_chat` (which has no sensible "empty" value to hand back) and every
//! write return a clean `"Sign in to use chats."` error.
//!
//! Migration note: the old flat, pre-§7-chatiso `conversations.db` (all
//! accounts mixed, S7-1/S7-2's shape) is now ORPHANED — its chats live
//! under none of the new per-account files and simply stop appearing,
//! under any account. This is deliberate and NOT auto-migrated: assigning
//! those rows to whichever account happens to sign in first would
//! recreate the exact cross-account leak this fix closes, since there is
//! no way to know which account actually owned any given row in the old
//! shared file. §7 has not shipped, so no real user data is affected —
//! existing dev/test chats simply need to be recreated per account.
//!
//! ## Command style
//! Mirrors `kpack.rs`: thin `#[tauri::command] async fn` -> `spawn_blocking`
//! around a pure [`ConvStore`] method, camelCase serde DTOs (Tauri's command
//! macro already maps a JS `camelCase` argument to the matching Rust
//! `snake_case` parameter, so command *parameters* need no `#[serde(rename)]`
//! — only the DTOs returned across IPC do). `ConvStore` is managed directly
//! (`app.manage(ConvStore::new(..))` in `main.rs`, not wrapped in an outer
//! `Arc`) — same as `kpack.rs`'s `EmbedderCache`/`Builds` — so a command
//! moves its `AppHandle` into the `spawn_blocking` closure and re-fetches
//! `app.state::<ConvStore>()` there rather than cloning an `Arc` up front.
//! `ConvStore` itself is `Send + Sync` (every field is: `Mutex<HashMap<..,
//! Arc<Mutex<Connection>>>>`, and SQLite is single-writer per connection
//! anyway, so one mutex-guarded connection per account is the right amount
//! of concurrency here — this is a desktop app, not a server).
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
//!   by `t4_delete_chat_leaves_no_orphan_fts_row` below). Because each
//!   account has its own `chat_fts` table (one per per-account database),
//!   `search_chats` never needs to filter by owner — a different account's
//!   messages are never in the table being searched.
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
//! A per-account database created before these two columns existed gets
//! them via a tiny defensive migration run at connection-open time (see
//! [`ConvStore::init_connection`]/[`ConvStore::migrate_chats_columns`]:
//! `PRAGMA table_info(chats)` + `ALTER TABLE ... ADD COLUMN` for whichever
//! is missing) — `CREATE TABLE IF NOT EXISTS` alone is a no-op against an
//! already-existing `chats` table of the OLD shape, so without this an
//! existing per-account DB would error on the next `INSERT`/`SELECT` that
//! touches either column.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::cloud::session::Cloud;

/// Schema (mirrors spec §7.1 exactly). `PRAGMA foreign_keys = ON` is set
/// here too, once per connection — it's a per-connection setting in
/// SQLite, and each per-account connection lives for the rest of this
/// process's lifetime once opened, so there's nowhere else it needs to be
/// re-set.
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

/// Where a fresh per-account database is opened from — see
/// [`ConvStore::conn_for`].
enum StoreRoot {
    /// `<dir>/<sanitized user_id>.db` — the real app (§7-chatiso).
    Dir(PathBuf),
    /// A fresh `:memory:` connection per account — only ever constructed
    /// by the `#[cfg(test)]`-gated `ConvStore::new_in_memory`, so a normal
    /// (non-test) build never constructs this variant; `#[allow(dead_code)]`
    /// rather than `#[cfg(test)]` on the variant itself, since `conn_for`'s
    /// `match` below needs this arm to exist in every build (cfg-gating the
    /// variant would make that match non-exhaustive outside `#[cfg(test)]`).
    #[allow(dead_code)]
    Memory,
}

/// The local conversation store: ONE SQLITE DATABASE PER ACCOUNT, lazily
/// opened and cached by [`ConvStore::conn_for`] — see the module doc
/// comment's "Per-account isolation" section for why (§7-chatiso).
pub struct ConvStore {
    root: StoreRoot,
    conns: Mutex<HashMap<String, Arc<Mutex<Connection>>>>,
}

impl ConvStore {
    /// `dir` is the conversations directory (`<app_data>/conversations/`
    /// in `main.rs`) — created on demand by `conn_for`, not here, so this
    /// never opens a database and never fails.
    pub fn new(dir: PathBuf) -> ConvStore {
        ConvStore {
            root: StoreRoot::Dir(dir),
            conns: Mutex::new(HashMap::new()),
        }
    }

    /// Tests only: `cleophis` is a binary crate with no other crate
    /// depending on it, so (unlike `kpack-core`'s cross-crate
    /// `MockEmbedder`, which needs a `test-util` Cargo feature to stay
    /// visible to a DIFFERENT crate's tests) a plain `#[cfg(test)]` is
    /// enough to keep this reachable from this module's own
    /// `#[cfg(test)] mod tests` below without leaving it as dead code in a
    /// normal (non-test) build. Each distinct `user_id` a test passes to a
    /// store method gets its own fresh `:memory:` connection the first
    /// time it's touched, cached exactly like a real per-account `.db`
    /// file — so a test simulating two accounts against ONE `ConvStore`
    /// (see `t9_per_account_isolation_...` below) gets two genuinely
    /// separate databases, exactly like production.
    #[cfg(test)]
    pub fn new_in_memory() -> ConvStore {
        ConvStore {
            root: StoreRoot::Memory,
            conns: Mutex::new(HashMap::new()),
        }
    }

    /// Returns the shared connection for `user_id`'s per-account database,
    /// opening (and, on first open, schema-initializing/migrating) it if
    /// this is the first call for that account this process. `user_id` is
    /// sanitized via [`account_dir_segment`] first — a malformed id is a
    /// hard `Err`, never a shared/escaped path (this is the enforcement
    /// point for §7-chatiso's per-account isolation). The whole
    /// check-or-create sequence runs under a single `conns` lock
    /// acquisition (never released and reacquired mid-check), so two
    /// concurrent first-accesses for the same account can't race to open
    /// two connections for it; the per-account `Arc` is cloned out and the
    /// `conns` (whole-map) lock is released before this returns — every
    /// actual read/write a caller does afterward only holds the
    /// PER-CONNECTION `Mutex`, not the whole-map lock, so different
    /// accounts' operations never block each other.
    fn conn_for(&self, user_id: &str) -> Result<Arc<Mutex<Connection>>, String> {
        let segment =
            account_dir_segment(user_id).ok_or_else(|| "invalid account id".to_string())?;

        let mut conns = self.conns.lock().unwrap();
        if let Some(conn) = conns.get(&segment) {
            return Ok(conn.clone());
        }

        let conn = match &self.root {
            StoreRoot::Dir(dir) => {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                Connection::open(dir.join(format!("{segment}.db"))).map_err(|e| e.to_string())?
            }
            StoreRoot::Memory => Connection::open_in_memory().map_err(|e| e.to_string())?,
        };
        Self::init_connection(&conn)?;

        let conn = Arc::new(Mutex::new(conn));
        conns.insert(segment, conn.clone());
        Ok(conn)
    }

    /// Schema init + defensive migration for a freshly opened per-account
    /// connection — factored out of `conn_for` so it runs identically
    /// whether the connection is a brand-new file, a pre-existing
    /// per-account file from before some later schema change, or a fresh
    /// `:memory:` connection.
    fn init_connection(conn: &Connection) -> Result<(), String> {
        conn.execute_batch(SCHEMA_SQL).map_err(|e| e.to_string())?;
        Self::migrate_chats_columns(conn)?;
        Ok(())
    }

    /// Defensive migration for a per-account database created before
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

    pub fn create_folder(&self, user_id: &str, name: &str) -> Result<FolderInfo, String> {
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();
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
    pub fn list_folders(&self, user_id: &str) -> Result<Vec<FolderInfo>, String> {
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();
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

    pub fn rename_folder(&self, user_id: &str, id: i64, name: &str) -> Result<(), String> {
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();
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
    pub fn delete_folder(&self, user_id: &str, id: i64) -> Result<(), String> {
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();
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
        user_id: &str,
        title: &str,
        folder_id: Option<i64>,
        mounted_packs: Option<Vec<String>>,
        model_id: &str,
        adapter_ids: Vec<String>,
    ) -> Result<ChatInfo, String> {
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();
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
    pub fn list_chats(&self, user_id: &str) -> Result<Vec<ChatInfo>, String> {
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();
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
    /// (conversation order). Because `user_id` resolves to a DIFFERENT
    /// database per account, `id` here can only ever match a row in the
    /// caller's own account — there's no cross-account id to leak, unlike
    /// a shared table where a guessed/enumerated id would need an explicit
    /// owner check.
    pub fn get_chat(&self, user_id: &str, id: i64) -> Result<ChatDetail, String> {
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();
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
    pub fn rename_chat(&self, user_id: &str, id: i64, title: &str) -> Result<(), String> {
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();
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
    pub fn delete_chat(&self, user_id: &str, id: i64) -> Result<(), String> {
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();
        conn.execute("DELETE FROM chats WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM chat_fts WHERE rowid = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn set_chat_pinned(&self, user_id: &str, id: i64, pinned: bool) -> Result<(), String> {
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();
        conn.execute(
            "UPDATE chats SET pinned = ?1 WHERE id = ?2",
            params![pinned, id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn set_chat_archived(&self, user_id: &str, id: i64, archived: bool) -> Result<(), String> {
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();
        conn.execute(
            "UPDATE chats SET archived = ?1 WHERE id = ?2",
            params![archived, id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn move_chat(&self, user_id: &str, id: i64, folder_id: Option<i64>) -> Result<(), String> {
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();
        conn.execute(
            "UPDATE chats SET folder_id = ?1 WHERE id = ?2",
            params![folder_id, id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn set_chat_packs(
        &self,
        user_id: &str,
        id: i64,
        mounted_packs: Vec<String>,
    ) -> Result<(), String> {
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();
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
        user_id: &str,
        chat_id: i64,
        role: &str,
        content: &str,
        citations: Option<Value>,
        tool_calls: Option<Value>,
    ) -> Result<MessageInfo, String> {
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();
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
    /// is the more intuitive UX than showing every chat. Only ever
    /// searches `user_id`'s own per-account `chat_fts` table — see the
    /// module doc comment.
    pub fn search_chats(&self, user_id: &str, query: &str) -> Result<Vec<ChatInfo>, String> {
        let safe_query = kpack_core::retrieve::safe_fts5_query(query);
        if safe_query.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();
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

    // ---- Export (§7.5) -----------------------------------------------

    /// Formats `id`'s chat + its messages into a single string per
    /// `format`, deterministically — same stored data, same bytes, every
    /// call. Goes through [`ConvStore::get_chat`] (the same
    /// `conn_for(user_id)` path every other method here uses), so this can
    /// only ever export the CALLER's own account's chat — see the module
    /// doc comment's "Per-account isolation" section; there is no way to
    /// pass another account's `id` through this and get anything back,
    /// exactly like `get_chat` itself (a mismatched `id` is `Err("no such
    /// chat")`, a same-numbered `id` that happens to exist in the caller's
    /// OWN database is that caller's own row, never the other account's).
    pub fn export_chat(
        &self,
        user_id: &str,
        id: i64,
        format: ExportFormat,
    ) -> Result<String, String> {
        let detail = self.get_chat(user_id, id)?;
        Ok(match format {
            ExportFormat::Markdown => export_markdown(&detail),
            ExportFormat::Json => export_json(&detail)?,
            ExportFormat::Txt => export_txt(&detail),
        })
    }
}

/// Sanitizes a session user id into a per-account database filename
/// segment. A VERBATIM MIRROR of `kpack.rs`'s module-private
/// `account_dir_segment` (see that function's doc comment there for the
/// full "reject, don't strip" rationale) — that one isn't `pub`/reachable
/// from this module, so this is a copy, not a shared import; keep the two
/// in sync if either ever changes. Accepts `user_id` unchanged if — and
/// only if — it's already clean: non-empty, every char
/// `[A-Za-z0-9_-]`. Anything else (empty, or containing so much as one
/// `.`/`/`/`\`/`:`/NUL/unicode/whitespace char) is a hard `None`, never a
/// stripped-down remainder — the only shape a Supabase-issued UUID ever
/// has, so real accounts are unaffected; a malformed/foreign id just gets
/// a hard `Err` via `ConvStore::conn_for` instead of a best-effort,
/// alias-prone or escaping filename.
fn account_dir_segment(user_id: &str) -> Option<String> {
    let is_clean = !user_id.is_empty()
        && user_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if is_clean {
        Some(user_id.to_string())
    } else {
        None
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

// ---- Export formatting (§7.5) -------------------------------------------
//
// Free functions, not `ConvStore` methods — they're pure `ChatDetail` ->
// `String` transforms with no I/O and nothing account-scoping-relevant left
// to do (that already happened in `export_chat`'s `get_chat` call), so
// there's no reason for them to carry a `&self`/`user_id`.

/// `msg.role` -> the label an export turn is prefixed with — `"You"`/
/// `"Assistant"` for the two roles every chat has today, a title-cased
/// fallback for any other role (there is no `"system"`/tool-role message
/// yet — `tool_calls` is unused per the module doc comment — but deriving
/// the label from the stored value rather than a hardcoded two-arm match
/// means an export never silently drops a future role's turns).
fn role_label(role: &str) -> String {
    match role {
        "user" => "You".to_string(),
        "assistant" => "Assistant".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => "Unknown".to_string(),
            }
        }
    }
}

/// One footnote line for a single stored citation object — `[n] docTitle ·
/// sectionPath · locator`, the SAME rendering `src/app.js`'s
/// `renderCitations` uses on-screen (§3a A4), so a Markdown export's
/// footnotes read exactly like the citations already shown in the chat UI.
/// Reads defensively via `Value::get`/`as_str`/`as_u64` rather than
/// deserializing into `kpack::CitationInfo` — `kpack` isn't a dependency of
/// this module, and this is the FRONT END's already-camelCase `citations`
/// JSON exactly as `append_message` stored it, not a fresh `Citation` from
/// a pack build — and skips a non-object entry or one missing `n` (`None`)
/// rather than failing the whole export over one malformed footnote.
fn format_citation_footnote(citation: &Value) -> Option<String> {
    let obj = citation.as_object()?;
    let n = obj.get("n").and_then(Value::as_u64)?;
    let doc_title = obj.get("docTitle").and_then(Value::as_str).unwrap_or("");
    let section_path = obj.get("sectionPath").and_then(Value::as_str).unwrap_or("");
    let locator = obj.get("locator").and_then(Value::as_str).unwrap_or("");
    let mut line = format!("[{n}] {doc_title}");
    if !section_path.is_empty() {
        line.push_str(" · ");
        line.push_str(section_path);
    }
    if !locator.is_empty() {
        line.push_str(" · ");
        line.push_str(locator);
    }
    Some(line)
}

/// Markdown export (§7.5 v1 default): title, created/updated dates, then
/// each turn as a role-labeled line with a `## {date}` header inserted
/// whenever a message's `created_at` date (the ISO string's date portion,
/// before the `T`) differs from the previous message's — a chat spanning
/// several days reads like a log, a same-day chat gets exactly one header.
/// A turn's citation footnotes are emitted directly under that turn (each
/// message's citations are already numbered `1..n` relative to THAT
/// message, mirroring the on-screen disclosure), so a reader never has to
/// jump to the end of the document for a turn's sources — and because this
/// reads straight from the stored `citations` JSON (never a live pack
/// query), the footnotes survive a pack being renamed/rebuilt/deleted after
/// the fact (§7.5's "citations persisted as displayed" invariant).
fn export_markdown(detail: &ChatDetail) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", detail.chat.title));
    out.push_str(&format!(
        "_Created {} · Updated {}_\n\n",
        detail.chat.created_at, detail.chat.updated_at
    ));

    let mut last_date: Option<&str> = None;
    for msg in &detail.messages {
        let date = msg.created_at.split('T').next().unwrap_or(&msg.created_at);
        if last_date != Some(date) {
            out.push_str(&format!("## {date}\n\n"));
            last_date = Some(date);
        }
        out.push_str(&format!(
            "**{}:** {}\n\n",
            role_label(&msg.role),
            msg.content
        ));

        if let Some(citations) = msg.citations.as_ref().and_then(Value::as_array) {
            let footnotes: Vec<String> = citations
                .iter()
                .filter_map(format_citation_footnote)
                .collect();
            if !footnotes.is_empty() {
                out.push_str(&footnotes.join("\n"));
                out.push_str("\n\n");
            }
        }
    }
    out
}

/// The versioned JSON export envelope (§7.5 v1) — `ChatInfo`/`MessageInfo`
/// already carry every column this format needs to preserve (citations,
/// tool_calls, timestamps, model_id/adapter_ids/mounted_packs) and already
/// serialize camelCase, so this just wraps them; no separate export-only
/// DTO to keep in sync with the real ones. `#[serde]` default field naming
/// (not `rename_all = "camelCase"`) is deliberate here: `export_schema` is
/// the literal key name the round-trip/import format specifies, not a
/// camelCase field that needs renaming.
#[derive(Serialize)]
struct ExportEnvelope<'a> {
    export_schema: u32,
    chat: &'a ChatInfo,
    messages: &'a [MessageInfo],
}

fn export_json(detail: &ChatDetail) -> Result<String, String> {
    let envelope = ExportEnvelope {
        export_schema: 1,
        chat: &detail.chat,
        messages: &detail.messages,
    };
    serde_json::to_string_pretty(&envelope).map_err(|e| e.to_string())
}

/// Plain-transcript export (§7.5 v1): title, then `You: …` / `Assistant: …`
/// lines — demo-friendly, no citations markup, no date headers (Markdown
/// already covers that "nice to have"; this format's whole point is being
/// the minimal one).
fn export_txt(detail: &ChatDetail) -> String {
    let mut out = String::new();
    out.push_str(&detail.chat.title);
    out.push_str("\n\n");
    for msg in &detail.messages {
        out.push_str(&format!("{}: {}\n\n", role_label(&msg.role), msg.content));
    }
    out
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

/// Output format for [`ConvStore::export_chat`] (§7.5). `Markdown` is the
/// v1 default (human-readable, citations rendered as footnotes exactly the
/// way the chat UI already shows them — see `format_citation_footnote`);
/// `Json` is the full-fidelity, versioned round-trip/import format (every
/// column, including `tool_calls` and the chat's inference provenance);
/// `Txt` is a minimal plain transcript with no citation markup, for a quick
/// paste. Not `Serialize`/`Deserialize` — it never crosses IPC itself; the
/// `export_chat_to_file` command below maps the front end's plain
/// `"markdown" | "json" | "txt"` string to this before calling
/// `export_chat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    Json,
    Txt,
}

// ==== Tauri commands =====================================================
//
// Thin wrappers only: each resolves the AUTHORITATIVE signed-in user id
// (never a front-end-supplied one — see `current_user_id` below and the
// module doc comment's "Per-account isolation" section), then runs a pure
// `ConvStore` method on a `spawn_blocking` thread (SQLite I/O is blocking),
// mapping a join failure to the same user-safe message `kpack.rs`/
// `cloud::commands` already use. `ConvStore`'s own methods do all the real
// work and are what the tests below exercise directly.

/// Only reachable if the blocking task itself panics or the runtime is
/// shutting down — mirrors `kpack.rs::JOIN_ERROR_MESSAGE`.
const JOIN_ERROR_MESSAGE: &str = "Something went wrong on this device. Please try again.";

/// Resolves the AUTHORITATIVE signed-in user id from the `Cloud` session
/// state (mirrors `kpack.rs`'s `packs_dir`, which resolves the same way
/// for knowledge packs — §7-chatiso applies A6's fix to conversations).
/// `None` means signed out; callers below decide per-command whether that
/// means an empty result or a clean "sign in" error, but in both cases the
/// store itself is never touched.
fn current_user_id(app: &AppHandle) -> Option<String> {
    app.state::<Arc<Cloud>>().current_user_id()
}

/// The error every WRITE command returns when signed out — a store method
/// is never called in that case. Reads decide individually (see each
/// command below): most return an empty result, `get_chat` (no sensible
/// "empty" `ChatDetail`) returns this same error.
fn sign_in_required() -> String {
    "Sign in to use chats.".to_string()
}

#[tauri::command]
pub async fn create_folder(name: String, app: AppHandle) -> Result<FolderInfo, String> {
    let user_id = current_user_id(&app).ok_or_else(sign_in_required)?;
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ConvStore>().create_folder(&user_id, &name)
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn list_folders(app: AppHandle) -> Result<Vec<FolderInfo>, String> {
    let Some(user_id) = current_user_id(&app) else {
        return Ok(Vec::new());
    };
    tauri::async_runtime::spawn_blocking(move || app.state::<ConvStore>().list_folders(&user_id))
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn rename_folder(id: i64, name: String, app: AppHandle) -> Result<(), String> {
    let user_id = current_user_id(&app).ok_or_else(sign_in_required)?;
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ConvStore>().rename_folder(&user_id, id, &name)
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn delete_folder(id: i64, app: AppHandle) -> Result<(), String> {
    let user_id = current_user_id(&app).ok_or_else(sign_in_required)?;
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ConvStore>().delete_folder(&user_id, id)
    })
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
    let user_id = current_user_id(&app).ok_or_else(sign_in_required)?;
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ConvStore>().create_chat(
            &user_id,
            &title,
            folder_id,
            mounted_packs,
            &model_id,
            adapter_ids,
        )
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn list_chats(app: AppHandle) -> Result<Vec<ChatInfo>, String> {
    let Some(user_id) = current_user_id(&app) else {
        return Ok(Vec::new());
    };
    tauri::async_runtime::spawn_blocking(move || app.state::<ConvStore>().list_chats(&user_id))
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

/// Unlike the other reads, signed-out is a clean `Err` here rather than an
/// empty result — there's no sensible "empty" `ChatDetail` to hand back
/// for a specific `id` (see the module doc comment).
#[tauri::command]
pub async fn get_chat(id: i64, app: AppHandle) -> Result<ChatDetail, String> {
    let user_id = current_user_id(&app).ok_or_else(sign_in_required)?;
    tauri::async_runtime::spawn_blocking(move || app.state::<ConvStore>().get_chat(&user_id, id))
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn rename_chat(id: i64, title: String, app: AppHandle) -> Result<(), String> {
    let user_id = current_user_id(&app).ok_or_else(sign_in_required)?;
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ConvStore>().rename_chat(&user_id, id, &title)
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn delete_chat(id: i64, app: AppHandle) -> Result<(), String> {
    let user_id = current_user_id(&app).ok_or_else(sign_in_required)?;
    tauri::async_runtime::spawn_blocking(move || app.state::<ConvStore>().delete_chat(&user_id, id))
        .await
        .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn set_chat_pinned(id: i64, pinned: bool, app: AppHandle) -> Result<(), String> {
    let user_id = current_user_id(&app).ok_or_else(sign_in_required)?;
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ConvStore>().set_chat_pinned(&user_id, id, pinned)
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn set_chat_archived(id: i64, archived: bool, app: AppHandle) -> Result<(), String> {
    let user_id = current_user_id(&app).ok_or_else(sign_in_required)?;
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ConvStore>()
            .set_chat_archived(&user_id, id, archived)
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn move_chat(id: i64, folder_id: Option<i64>, app: AppHandle) -> Result<(), String> {
    let user_id = current_user_id(&app).ok_or_else(sign_in_required)?;
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ConvStore>().move_chat(&user_id, id, folder_id)
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn set_chat_packs(
    id: i64,
    mounted_packs: Vec<String>,
    app: AppHandle,
) -> Result<(), String> {
    let user_id = current_user_id(&app).ok_or_else(sign_in_required)?;
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ConvStore>()
            .set_chat_packs(&user_id, id, mounted_packs)
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
    let user_id = current_user_id(&app).ok_or_else(sign_in_required)?;
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ConvStore>().append_message(
            &user_id,
            chat_id,
            &role,
            &content,
            citations,
            tool_calls,
        )
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[tauri::command]
pub async fn search_chats(query: String, app: AppHandle) -> Result<Vec<ChatInfo>, String> {
    let Some(user_id) = current_user_id(&app) else {
        return Ok(Vec::new());
    };
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ConvStore>().search_chats(&user_id, &query)
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

/// Formats + writes `id`'s chat to `path` (the front end's OS save-dialog
/// choice — see `src/app.js`'s export flow) in the requested `format`.
/// Resolves the AUTHORITATIVE signed-in user id exactly like every other
/// command in this module (never a front-end-supplied one) — signed-out is
/// a clean `Err("Sign in to export.")`, the store is never touched.
/// `format` is a plain string over IPC (`"markdown" | "json" | "txt"`,
/// matching `src/app.js`'s export menu) rather than `ExportFormat` itself —
/// a `serde`-derived enum here would ALSO accept a malformed shape like
/// `{"Markdown": null}` from a compromised/buggy renderer, where a `match`
/// on a plain string is exactly as strict as this command needs and matches
/// every other command's primitive-typed IPC parameters. One command that
/// both formats AND writes, rather than a general write-to-path primitive
/// — the only file this can ever write is `path` (the user's own OS save
/// choice), and the only content is `export_chat`'s own deterministic
/// output, so no broader write capability is exposed here.
#[tauri::command]
pub async fn export_chat_to_file(
    id: i64,
    format: String,
    path: String,
    app: AppHandle,
) -> Result<(), String> {
    let user_id = current_user_id(&app).ok_or_else(|| "Sign in to export.".to_string())?;
    let fmt = match format.as_str() {
        "markdown" => ExportFormat::Markdown,
        "json" => ExportFormat::Json,
        "txt" => ExportFormat::Txt,
        other => return Err(format!("Unknown export format: {other}")),
    };
    tauri::async_runtime::spawn_blocking(move || {
        let content = app.state::<ConvStore>().export_chat(&user_id, id, fmt)?;
        std::fs::write(&path, content).map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The fixed account every single-account test below uses — these
    /// tests exercise one account's CRUD/fts/delete/migration behavior,
    /// unchanged by §7-chatiso; `t9`/`t10` below are the ones that actually
    /// exercise multiple accounts.
    const USER: &str = "tester";

    /// create_chat -> append 2 messages (one with citations) -> get_chat
    /// returns the chat + both messages in conversation order, citations
    /// round-trip intact.
    #[test]
    fn t1_get_chat_returns_chat_and_messages_in_order_with_citations() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "My chat", None, None, "hero-llama", vec![])
            .unwrap();

        let m1 = store
            .append_message(USER, chat.id, "user", "hello", None, None)
            .unwrap();
        let citations = json!([{"n": 1, "docTitle": "Doc"}]);
        let m2 = store
            .append_message(
                USER,
                chat.id,
                "assistant",
                "hi there",
                Some(citations.clone()),
                None,
            )
            .unwrap();

        let detail = store.get_chat(USER, chat.id).unwrap();
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
        let store = ConvStore::new_in_memory();
        let older = store
            .create_chat(USER, "Older", None, None, "hero-llama", vec![])
            .unwrap();
        let newer = store
            .create_chat(USER, "Newer", None, None, "hero-llama", vec![])
            .unwrap();
        store.set_chat_pinned(USER, older.id, true).unwrap();
        store.set_chat_archived(USER, newer.id, true).unwrap();

        let chats = store.list_chats(USER).unwrap();
        assert_eq!(chats[0].id, older.id, "the pinned chat must sort first");
        assert!(chats[0].pinned);
        let newer_entry = chats.iter().find(|c| c.id == newer.id).unwrap();
        assert!(newer_entry.archived);
    }

    /// folders: create -> move a chat in -> list; delete_folder unfiles its
    /// chats (folder_id NULL) rather than deleting them.
    #[test]
    fn t3_delete_folder_unfiles_chats_instead_of_deleting_them() {
        let store = ConvStore::new_in_memory();
        let folder = store.create_folder(USER, "Work").unwrap();
        let chat = store
            .create_chat(USER, "A chat", None, None, "hero-llama", vec![])
            .unwrap();
        store.move_chat(USER, chat.id, Some(folder.id)).unwrap();

        let folders = store.list_folders(USER).unwrap();
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].id, folder.id);
        let moved = store.get_chat(USER, chat.id).unwrap().chat;
        assert_eq!(moved.folder_id, Some(folder.id));

        store.delete_folder(USER, folder.id).unwrap();

        assert!(store.list_folders(USER).unwrap().is_empty());
        let survived = store.get_chat(USER, chat.id).unwrap().chat;
        assert_eq!(survived.folder_id, None, "the chat must survive, unfiled");
    }

    /// delete_chat: messages are gone (FK cascade) AND no fts rows survive
    /// — search for the deleted chat's content finds nothing (§7.5).
    #[test]
    fn t4_delete_chat_leaves_no_orphan_fts_row() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "Doomed chat", None, None, "hero-llama", vec![])
            .unwrap();
        store
            .append_message(USER, chat.id, "user", "unique_marker_xyz", None, None)
            .unwrap();

        assert_eq!(
            store.search_chats(USER, "unique_marker_xyz").unwrap().len(),
            1
        );

        store.delete_chat(USER, chat.id).unwrap();

        assert!(
            store.get_chat(USER, chat.id).is_err(),
            "the chat itself is gone"
        );
        assert!(
            store
                .search_chats(USER, "unique_marker_xyz")
                .unwrap()
                .is_empty(),
            "no orphan fts row should match the deleted chat's content"
        );
        assert!(
            store.search_chats(USER, "Doomed").unwrap().is_empty(),
            "no orphan fts row should match the deleted chat's title either"
        );
    }

    /// search_chats finds a chat by a word in its title and by a word in a
    /// message's content; a deleted chat never appears in results.
    #[test]
    fn t5_search_chats_matches_title_and_message_content_not_deleted_chats() {
        let store = ConvStore::new_in_memory();
        let by_title = store
            .create_chat(USER, "Vitamin K research", None, None, "hero-llama", vec![])
            .unwrap();
        let by_content = store
            .create_chat(USER, "Untitled", None, None, "hero-llama", vec![])
            .unwrap();
        store
            .append_message(
                USER,
                by_content.id,
                "user",
                "tell me about blood clotting",
                None,
                None,
            )
            .unwrap();
        let gone = store
            .create_chat(USER, "clotting notes", None, None, "hero-llama", vec![])
            .unwrap();
        store.delete_chat(USER, gone.id).unwrap();

        let title_hits = store.search_chats(USER, "Vitamin").unwrap();
        assert_eq!(title_hits.len(), 1);
        assert_eq!(title_hits[0].id, by_title.id);

        let content_hits = store.search_chats(USER, "clotting").unwrap();
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
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "Timing", None, None, "hero-llama", vec![])
            .unwrap();
        assert!(!chat.created_at.is_empty());
        assert!(!chat.updated_at.is_empty());
        assert_eq!(chat.created_at, chat.updated_at);

        std::thread::sleep(std::time::Duration::from_millis(20));
        store
            .append_message(USER, chat.id, "user", "first", None, None)
            .unwrap();
        let after_first = store.get_chat(USER, chat.id).unwrap().chat;
        assert!(after_first.updated_at > chat.updated_at);

        std::thread::sleep(std::time::Duration::from_millis(20));
        store
            .append_message(USER, chat.id, "assistant", "second", None, None)
            .unwrap();
        let after_second = store.get_chat(USER, chat.id).unwrap().chat;
        assert!(after_second.updated_at > after_first.updated_at);
    }

    /// create_chat's model_id/adapter_ids provenance round-trips through
    /// both get_chat and list_chats (forward-compat for the future
    /// LoRA-adapter track — see the module doc comment).
    #[test]
    fn t7_model_and_adapter_provenance_round_trips() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(
                USER,
                "Specialist chat",
                None,
                None,
                "hero-llama",
                vec!["med-adapter-v1".to_string()],
            )
            .unwrap();
        assert_eq!(chat.model_id, "hero-llama");
        assert_eq!(chat.adapter_ids, vec!["med-adapter-v1".to_string()]);

        let fetched = store.get_chat(USER, chat.id).unwrap().chat;
        assert_eq!(fetched.model_id, "hero-llama");
        assert_eq!(fetched.adapter_ids, vec!["med-adapter-v1".to_string()]);

        let listed = store.list_chats(USER).unwrap();
        let listed_chat = listed.iter().find(|c| c.id == chat.id).unwrap();
        assert_eq!(listed_chat.model_id, "hero-llama");
        assert_eq!(listed_chat.adapter_ids, vec!["med-adapter-v1".to_string()]);
    }

    /// Defensive migration: a per-account `.db` file created with the OLD
    /// (pre-model_id/adapter_ids) `chats` shape — hand-built directly on
    /// disk here, bypassing `SCHEMA_SQL` entirely — still opens cleanly the
    /// first time `ConvStore::conn_for` touches it, and `create_chat`
    /// against it succeeds with the new columns backfilled to their
    /// defaults rather than erroring on the missing columns. This exercises
    /// the REAL open path (`ConvStore::new` + a real directory), not just
    /// `migrate_chats_columns` in isolation.
    #[test]
    fn t8_migrates_an_existing_per_account_db_missing_the_new_columns() {
        let dir = std::env::temp_dir().join(format!(
            "cleophis-convstore-migrate-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join(format!("{USER}.db"));
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE chats (
                    id INTEGER PRIMARY KEY, folder_id INTEGER REFERENCES folders(id),
                    title TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
                    pinned INTEGER NOT NULL DEFAULT 0, archived INTEGER NOT NULL DEFAULT 0,
                    mounted_packs TEXT
                );",
            )
            .unwrap();
        }

        let store = ConvStore::new(dir.clone());
        let chat = store
            .create_chat(USER, "Migrated", None, None, "hero-llama", vec![])
            .unwrap();
        assert_eq!(chat.model_id, "hero-llama");
        assert!(chat.adapter_ids.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// THE SECURITY ASSERTION (§7-chatiso): with ONE `ConvStore`, a chat
    /// created for `"acct-a"` is invisible to `"acct-b"` in every read
    /// path — `list_chats` only ever shows the caller's own chat, and
    /// `search_chats` never surfaces the other account's content.
    /// Un-skippable: this is the exact bug report (bouncing between 3
    /// accounts, seeing every account's chats).
    ///
    /// `get_chat` gets a NOTE, not a plain `.is_err()`: per-account chat
    /// ids are LOCAL to each account's own database (each starts its own
    /// id sequence at 1), so `a_chat.id == b_chat.id` here is EXPECTED
    /// (both are each account's first-ever chat) — that numeric coincidence
    /// is harmless by design, not a leak, so asserting `get_chat("acct-b",
    /// a_chat.id).is_err()` would be asserting the wrong thing (it
    /// legitimately succeeds — it just returns B's OWN row at that id
    /// number, never A's). The actual security property — proven below —
    /// is content-level: whether or not the id numbers collide, B's
    /// `get_chat` call must never come back with A's title or message
    /// content, and vice versa.
    #[test]
    fn t9_per_account_isolation_a_chat_created_for_one_account_is_invisible_to_another() {
        let store = ConvStore::new_in_memory();
        let a_chat = store
            .create_chat(
                "acct-a",
                "A's private chat",
                None,
                None,
                "hero-llama",
                vec![],
            )
            .unwrap();
        store
            .append_message(
                "acct-a",
                a_chat.id,
                "user",
                "a_secret_marker_only_a_should_see",
                None,
                None,
            )
            .unwrap();
        let b_chat = store
            .create_chat(
                "acct-b",
                "B's private chat",
                None,
                None,
                "hero-llama",
                vec![],
            )
            .unwrap();

        let a_list = store.list_chats("acct-a").unwrap();
        assert_eq!(a_list.len(), 1, "A must see only A's own chat");
        assert_eq!(a_list[0].title, "A's private chat");

        let b_list = store.list_chats("acct-b").unwrap();
        assert_eq!(b_list.len(), 1, "B must see only B's own chat");
        assert_eq!(b_list[0].title, "B's private chat");

        // B looking up A's chat id (which — see the test's doc comment —
        // is very likely also B's own first chat id): either B has no row
        // there (Err) or, since it's B's own database, gets B's own row
        // back — A's title/content must never come back either way.
        if let Ok(detail) = store.get_chat("acct-b", a_chat.id) {
            assert_ne!(
                detail.chat.title, "A's private chat",
                "B's get_chat must never return A's chat"
            );
            assert!(
                !detail
                    .messages
                    .iter()
                    .any(|m| m.content.contains("a_secret_marker_only_a_should_see")),
                "B's get_chat must never return A's message content"
            );
        }
        if let Ok(detail) = store.get_chat("acct-a", b_chat.id) {
            assert_ne!(
                detail.chat.title, "B's private chat",
                "A's get_chat must never return B's chat"
            );
        }

        assert!(
            store
                .search_chats("acct-b", "a_secret_marker_only_a_should_see")
                .unwrap()
                .is_empty(),
            "B's search must never surface A's message content"
        );
        assert!(
            store
                .search_chats("acct-b", "private")
                .unwrap()
                .iter()
                .all(|c| c.title != "A's private chat"),
            "B's search must never surface A's chat, even for a word both titles share"
        );
    }

    /// The account-segment sanitizer: a clean id (uuid-shaped) passes
    /// unchanged; empty, a traversal attempt, and an embedded separator
    /// all reject — never a shared or escaped path.
    #[test]
    fn t10_account_dir_segment_accepts_clean_ids_rejects_the_rest() {
        assert_eq!(
            account_dir_segment("3fae1c02-9c4e-4b1a-8a2e-1f2b3c4d5e6f"),
            Some("3fae1c02-9c4e-4b1a-8a2e-1f2b3c4d5e6f".to_string())
        );
        assert_eq!(account_dir_segment(""), None);
        assert_eq!(account_dir_segment("../x"), None);
        assert_eq!(account_dir_segment("a/b"), None);
        assert_eq!(account_dir_segment("a\\b"), None);
        assert_eq!(account_dir_segment("a:b"), None);
    }

    /// Markdown export: title, both turns' role labels + content, and the
    /// citation footnote (docTitle + locator) for the message that carries
    /// citations — the A4 `CitationInfo` shape, stored verbatim by
    /// `append_message`.
    #[test]
    fn t11_export_chat_markdown_includes_title_turns_and_citation_footnote() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "Export me", None, None, "hero-llama", vec![])
            .unwrap();
        store
            .append_message(USER, chat.id, "user", "What is vitamin K?", None, None)
            .unwrap();
        let citations = json!([{
            "n": 1, "packId": "p1", "chunkId": 3,
            "docTitle": "Hematology 101", "sectionPath": "Ch. 4", "locator": "p.12"
        }]);
        store
            .append_message(
                USER,
                chat.id,
                "assistant",
                "It helps blood clot.",
                Some(citations),
                None,
            )
            .unwrap();

        let md = store
            .export_chat(USER, chat.id, ExportFormat::Markdown)
            .unwrap();
        assert!(md.contains("Export me"), "title must appear");
        assert!(md.contains("**You:** What is vitamin K?"));
        assert!(md.contains("**Assistant:** It helps blood clot."));
        assert!(
            md.contains("Hematology 101") && md.contains("p.12"),
            "the citation footnote (docTitle/locator) must appear:\n{md}"
        );
    }

    /// JSON export: `export_schema: 1`, both messages (with citations
    /// intact) and the chat's `model_id` all round-trip.
    #[test]
    fn t12_export_chat_json_round_trips_schema_messages_citations_and_model_id() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "JSON export", None, None, "hero-llama", vec![])
            .unwrap();
        store
            .append_message(USER, chat.id, "user", "hi", None, None)
            .unwrap();
        let citations = json!([{"n": 1, "docTitle": "Doc"}]);
        store
            .append_message(
                USER,
                chat.id,
                "assistant",
                "hello",
                Some(citations.clone()),
                None,
            )
            .unwrap();

        let raw = store.export_chat(USER, chat.id, ExportFormat::Json).unwrap();
        let parsed: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["export_schema"], 1);
        assert_eq!(parsed["chat"]["modelId"], "hero-llama");
        let messages = parsed["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["content"], "hi");
        assert_eq!(messages[1]["content"], "hello");
        assert_eq!(messages[1]["citations"], citations);
    }

    /// TXT export: a plain transcript (title + `You:`/`Assistant:` lines),
    /// no citation markup even when the message carries citations.
    #[test]
    fn t13_export_chat_txt_is_plain_transcript_without_citation_markup() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "Txt export", None, None, "hero-llama", vec![])
            .unwrap();
        store
            .append_message(USER, chat.id, "user", "hi", None, None)
            .unwrap();
        let citations = json!([{"n": 1, "docTitle": "Doc", "locator": "p.1"}]);
        store
            .append_message(USER, chat.id, "assistant", "hello", Some(citations), None)
            .unwrap();

        let txt = store.export_chat(USER, chat.id, ExportFormat::Txt).unwrap();
        assert!(txt.contains("Txt export"));
        assert!(txt.contains("You: hi"));
        assert!(txt.contains("Assistant: hello"));
        assert!(!txt.contains("Doc"), "TXT must carry no citation markup");
        assert!(!txt.contains("p.1"), "TXT must carry no citation markup");
    }

    /// Same chat -> identical bytes across two calls, for every format
    /// (§7.5's determinism requirement).
    #[test]
    fn t14_export_chat_is_deterministic_across_calls() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "Deterministic", None, None, "hero-llama", vec![])
            .unwrap();
        store
            .append_message(USER, chat.id, "user", "one", None, None)
            .unwrap();
        store
            .append_message(USER, chat.id, "assistant", "two", None, None)
            .unwrap();

        for format in [ExportFormat::Markdown, ExportFormat::Json, ExportFormat::Txt] {
            let a = store.export_chat(USER, chat.id, format).unwrap();
            let b = store.export_chat(USER, chat.id, format).unwrap();
            assert_eq!(a, b, "{format:?} export must be byte-identical across calls");
        }
    }

    /// Account-scoping (mirrors `t9`): B cannot export A's chat.
    #[test]
    fn t15_export_chat_is_account_scoped_b_cannot_export_as_chat() {
        let store = ConvStore::new_in_memory();
        let a_chat = store
            .create_chat(
                "acct-a",
                "A's private chat",
                None,
                None,
                "hero-llama",
                vec![],
            )
            .unwrap();
        store
            .append_message(
                "acct-a",
                a_chat.id,
                "user",
                "a_secret_marker_only_a_should_see",
                None,
                None,
            )
            .unwrap();
        store
            .create_chat(
                "acct-b",
                "B's private chat",
                None,
                None,
                "hero-llama",
                vec![],
            )
            .unwrap();

        // B's own database very likely also has an id-1 chat (its own first
        // chat) — see t9's doc comment on why that numeric coincidence is
        // expected and harmless. Either B gets a hard Err, or (since it's
        // B's own database) B's own export back — A's content must never
        // appear either way.
        match store.export_chat("acct-b", a_chat.id, ExportFormat::Markdown) {
            Err(_) => {}
            Ok(content) => {
                assert!(
                    !content.contains("a_secret_marker_only_a_should_see"),
                    "B's export must never contain A's message content"
                );
                assert!(
                    !content.contains("A's private chat"),
                    "B's export must never contain A's chat title"
                );
            }
        }
    }
}
