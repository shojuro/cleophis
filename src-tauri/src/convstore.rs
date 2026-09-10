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
//!
//! ## Auto-title (§7 S7-5)
//! `chats.title_auto` (`1` = the title may still be replaced by a
//! model-generated one; `0` = the user renamed the chat, never overwrite
//! it again) is the guard behind [`ConvStore::auto_title_chat`]: after the
//! first complete exchange in a NEW chat, `src/app.js` asks the local
//! model for a concise title and calls `auto_title_chat`, which only
//! applies it while `title_auto = 1` — a plain `WHERE title_auto = 1` on
//! the `UPDATE`, so a user rename (which flips the flag to `0` in
//! [`ConvStore::rename_chat`]) always wins over a late/racing auto-title
//! by construction, with no ordering logic needed on either side.
//! `create_chat` never sets `title_auto` explicitly — the column's
//! `DEFAULT 1` covers every new chat — and `auto_title_chat` itself never
//! touches the flag either way (it only ever CONSUMES the `1` state, never
//! sets or clears it). Migrated the same defensive way as `model_id`/
//! `adapter_ids` above.
//!
//! ## Supervised triage (P2.1 / P2.5)
//! A `supervised` catalog entry's replies pass through the product guard
//! (`src/triage/guard.js`) before they are shown, and three `messages`
//! columns keep that audit trail: `guard` (the verdict as JSON — route,
//! banner, the RAW reply, what was stripped, the detector pin),
//! `confirmed_route` and `confirmed_at` (the health worker's decision, via
//! [`ConvStore::confirm_route`]). All three are `NULL` for every ordinary
//! message, so nothing about the tutor hero's rows changes — including
//! their IPC payloads, since all three are `skip_serializing_if`-skipped
//! when absent, exactly like `partial`. Migrated the same defensive way as
//! the `chats` columns above.
//!
//! The verdict is stored, never recomputed: the human's confirmation lands
//! in its own columns rather than overwriting `guard`, which is what lets
//! [`ConvStore::export_triage_log`] report `overridden` (the confirmed
//! route differing from the model's) at all.
//!
//! ### One assistant row per turn
//! On mobile the front end's `append_message` and the Rust checkpointer
//! both persisted the reply, so a supervised turn ended up as TWO assistant
//! rows — the checkpointer's finished partial and the front end's insert.
//! `append_message` now UPGRADES this chat's in-flight partial row when the
//! appended role is `assistant`, so an append arriving while the turn is
//! still in flight lands ON the row the checkpointer already holds. A
//! desktop append, where nothing ever checkpoints, finds no partial row and
//! inserts exactly as before.
//!
//! That upgrade is not enough on its own for the mobile path, because
//! `chat_cmds`'s `settle` FINALIZES the checkpoint row (clearing `partial`)
//! before it sends the `Done` event that hands the front end the reply — so
//! when the verdict finally exists there is no in-flight row left to find.
//! `Done` therefore gains the finalized row's id (Task 6's half — it does
//! not carry one yet), and the front end calls [`ConvStore::attach_guard`]
//! on that id instead of appending a second row. That call also replaces
//! the row's RAW content with the guard's `displayText`, so the single
//! surviving row holds what the user saw while the guard column keeps what
//! the model said. The two together mean exactly one assistant row per
//! turn on either platform, whichever call arrives with the verdict.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection, OptionalExtension};
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
  adapter_ids TEXT NOT NULL DEFAULT '[]',
  title_auto INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY, chat_id INTEGER NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
  role TEXT NOT NULL, content TEXT NOT NULL,
  citations TEXT,
  tool_calls TEXT,
  created_at TEXT NOT NULL,
  partial INTEGER NOT NULL DEFAULT 0,
  guard TEXT,
  confirmed_route TEXT,
  confirmed_at TEXT
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
const MESSAGE_COLUMNS: &str =
    "id, role, content, citations, tool_calls, created_at, partial, guard, confirmed_route, \
     confirmed_at";

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
        Self::migrate_messages_columns(conn)?;
        Ok(())
    }

    /// Defensive migration for a per-account database created before
    /// `chats.model_id`/`chats.adapter_ids`/`chats.title_auto` existed —
    /// see the module doc comment's "Inference provenance" section and
    /// (for `title_auto`) the "auto-title" section. `CREATE TABLE IF NOT
    /// EXISTS` in `SCHEMA_SQL` is a no-op against an already-existing
    /// `chats` table regardless of its column shape, so this checks
    /// `PRAGMA table_info(chats)` for each of the three columns and
    /// `ALTER TABLE ... ADD COLUMN`s whichever is missing. Every default is
    /// a plain constant literal (`''`/`'[]'`/`1`), which SQLite allows on a
    /// `NOT NULL ADD COLUMN` (it backfills every existing row with that
    /// literal) — only a non-constant default like `CURRENT_TIMESTAMP`
    /// would be rejected there. A brand-new database (via `SCHEMA_SQL`'s
    /// `CREATE TABLE`) already has all three columns, so this is a no-op
    /// for it. An existing chat backfilled to `title_auto = 1` is harmless
    /// even though it's already past its first turn — auto-title only ever
    /// fires on a brand-new chat's first exchange (see `src/app.js`'s
    /// `isFirstExchange`), which such a chat has long since passed, so the
    /// backfilled `1` is never actually acted on for it.
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
        if !existing.contains("title_auto") {
            conn.execute(
                "ALTER TABLE chats ADD COLUMN title_auto INTEGER NOT NULL DEFAULT 1",
                [],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// The `messages` counterpart of [`Self::migrate_chats_columns`], same
    /// mechanism and same reasoning: `PRAGMA table_info` then `ALTER TABLE ...
    /// ADD COLUMN` for whatever is missing. `0` is a constant literal, which
    /// SQLite accepts on a `NOT NULL ADD COLUMN` and backfills into every
    /// existing row — and backfilling `partial = 0` is exactly right, since
    /// every message written before this column existed was a completed one.
    ///
    /// The three supervised-triage columns (`guard`/`confirmed_route`/
    /// `confirmed_at`, see the module doc comment's "Supervised triage"
    /// section) migrate the same way. All three are nullable with no
    /// `DEFAULT`, so an existing row backfills to `NULL` — which reads back
    /// as `None` and is exactly right: a message written before the guard
    /// existed carries no verdict and no health-worker confirmation.
    fn migrate_messages_columns(conn: &Connection) -> Result<(), String> {
        let mut existing = std::collections::HashSet::new();
        {
            let mut stmt = conn
                .prepare("PRAGMA table_info(messages)")
                .map_err(|e| e.to_string())?;
            let names = stmt
                .query_map([], |row| row.get::<_, String>(1))
                .map_err(|e| e.to_string())?;
            for name in names {
                existing.insert(name.map_err(|e| e.to_string())?);
            }
        }
        if !existing.contains("partial") {
            conn.execute(
                "ALTER TABLE messages ADD COLUMN partial INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(|e| e.to_string())?;
        }
        for (name, ddl) in [
            ("guard", "ALTER TABLE messages ADD COLUMN guard TEXT"),
            (
                "confirmed_route",
                "ALTER TABLE messages ADD COLUMN confirmed_route TEXT",
            ),
            (
                "confirmed_at",
                "ALTER TABLE messages ADD COLUMN confirmed_at TEXT",
            ),
        ] {
            if !existing.contains(name) {
                conn.execute(ddl, []).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    // ---- Partial-turn checkpointing (kill-restore gate) ---------------
    //
    // Generation on mobile can be killed at any moment — the OS reclaims a
    // backgrounded app without ceremony — so the assistant's text is
    // checkpointed to the database as it streams rather than only at the end.
    //
    // The checkpoint is a real `messages` row carrying `partial = 1`, cleared
    // on finalize. It is deliberately NOT an in-place update of an ordinary
    // message row: recovery has to distinguish "truncated because the process
    // died" from "completed", and an in-place update destroys exactly that
    // distinction — a truncated row and a short-but-finished row become
    // indistinguishable. §11's kill-restore test asserts
    // truncated-but-uncorrupted, which is only assertable when truncation is a
    // recorded state rather than something inferred from content.

    /// Write or advance the in-flight assistant turn for `chat_id`.
    ///
    /// At most one partial row exists per chat: the first call inserts, later
    /// calls overwrite that row's content. Returns its id.
    // Mobile-only: only `chat_stream`'s cfg(mobile) path checkpoints, because
    // desktop's transport streams through the sidecar and never writes an
    // in-flight row. Dead on desktop by platform fact, not by oversight.
    #[cfg_attr(desktop, allow(dead_code))]
    pub fn checkpoint_partial(
        &self,
        user_id: &str,
        chat_id: i64,
        content: &str,
    ) -> Result<i64, String> {
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();

        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM messages WHERE chat_id = ?1 AND partial = 1 \
                 ORDER BY id DESC LIMIT 1",
                params![chat_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;

        if let Some(id) = existing {
            conn.execute(
                "UPDATE messages SET content = ?1 WHERE id = ?2",
                params![content, id],
            )
            .map_err(|e| e.to_string())?;
            return Ok(id);
        }

        conn.execute(
            "INSERT INTO messages (chat_id, role, content, citations, tool_calls, created_at, partial)
             VALUES (?1, 'assistant', ?2, NULL, NULL, ?3, 1)",
            params![chat_id, content, now_iso()],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    }

    /// Promote the in-flight row to a finished message: final content, and
    /// `partial` cleared so recovery stops treating it as truncated.
    // Mobile-only: only `chat_stream`'s cfg(mobile) path checkpoints, because
    // desktop's transport streams through the sidecar and never writes an
    // in-flight row. Dead on desktop by platform fact, not by oversight.
    #[cfg_attr(desktop, allow(dead_code))]
    pub fn finalize_partial(
        &self,
        user_id: &str,
        message_id: i64,
        content: &str,
        citations: Option<Value>,
        tool_calls: Option<Value>,
    ) -> Result<(), String> {
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();
        conn.execute(
            "UPDATE messages SET content = ?1, citations = ?2, tool_calls = ?3, partial = 0 \
             WHERE id = ?4",
            params![
                content,
                citations.as_ref().map(|v| v.to_string()),
                tool_calls.as_ref().map(|v| v.to_string()),
                message_id
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Drop the in-flight row for `chat_id`, if any — used when a turn is
    /// cancelled before producing anything worth keeping.
    // Mobile-only: only `chat_stream`'s cfg(mobile) path checkpoints, because
    // desktop's transport streams through the sidecar and never writes an
    // in-flight row. Dead on desktop by platform fact, not by oversight.
    #[cfg_attr(desktop, allow(dead_code))]
    pub fn discard_partial(&self, user_id: &str, chat_id: i64) -> Result<(), String> {
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();
        conn.execute(
            "DELETE FROM messages WHERE chat_id = ?1 AND partial = 1",
            params![chat_id],
        )
        .map_err(|e| e.to_string())?;
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

    /// Bumps `updated_at` (spec §7.4), updates the fts title row, and sets
    /// `title_auto = 0` — the user has taken control of the title, so
    /// [`ConvStore::auto_title_chat`]'s `WHERE title_auto = 1` guard must
    /// never overwrite it again (§7 S7-5: a user rename always wins over a
    /// late auto-title, by construction).
    pub fn rename_chat(&self, user_id: &str, id: i64, title: &str) -> Result<(), String> {
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();
        let now = now_iso();
        conn.execute(
            "UPDATE chats SET title = ?1, updated_at = ?2, title_auto = 0 WHERE id = ?3",
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

    /// Applies a model-generated title, but ONLY if the chat hasn't been
    /// user-renamed since creation (§7 S7-5 — see the module doc comment's
    /// "auto-title" section). Trims `title` and caps it to 80 chars
    /// defensively — the FE (`maybeAutoTitle` in `src/app.js`) already
    /// sanitizes/caps to ~60 chars before calling this, so this is
    /// defense-in-depth, not the primary guard. Returns `Ok(false)`
    /// (nothing applied) for: an empty/whitespace-only `title`; an `id`
    /// that doesn't match any row in `user_id`'s own database (a foreign
    /// account's id simply isn't a row here — see the module doc comment's
    /// "Per-account isolation" section, same reasoning as every other
    /// method, no separate owner check needed); or a chat whose
    /// `title_auto` is already 0 (the user renamed it — `rename_chat`
    /// cleared the flag, and this method never sets it back). The `AND
    /// title_auto = 1` in the `UPDATE` below IS that guard — a matching
    /// row only updates if it's still in the auto-titled state, so a
    /// user rename that raced ahead of this call always wins. Does NOT
    /// touch `title_auto` itself (it stays 1) — unlike `rename_chat`,
    /// this path never claims ownership of the title.
    pub fn auto_title_chat(&self, user_id: &str, id: i64, title: &str) -> Result<bool, String> {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Ok(false);
        }
        let capped: String = trimmed.chars().take(80).collect();

        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();
        let now = now_iso();
        let changed = conn
            .execute(
                "UPDATE chats SET title = ?1, updated_at = ?2 WHERE id = ?3 AND title_auto = 1",
                params![capped, now, id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Ok(false);
        }
        conn.execute(
            "UPDATE chat_fts SET title = ?1 WHERE rowid = ?2",
            params![capped, id],
        )
        .map_err(|e| e.to_string())?;
        Ok(true)
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
    ///
    /// `guard` is the product guard's verdict for a supervised reply (see
    /// [`MessageInfo::guard`]); `None` for every ordinary message, which is
    /// every message the tutor hero ever writes.
    ///
    /// An ASSISTANT append UPGRADES this chat's in-flight partial row when
    /// one exists rather than inserting a second row — see the module doc
    /// comment's "Supervised triage" section for the double-write this
    /// closes.
    pub fn append_message(
        &self,
        user_id: &str,
        chat_id: i64,
        role: &str,
        content: &str,
        citations: Option<Value>,
        tool_calls: Option<Value>,
        guard: Option<Value>,
    ) -> Result<MessageInfo, String> {
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();
        let now = now_iso();
        let citations_json = citations.as_ref().map(|v| v.to_string());
        let tool_calls_json = tool_calls.as_ref().map(|v| v.to_string());
        let guard_json = guard.as_ref().map(|v| v.to_string());

        // The mobile checkpointer may already hold this turn as a partial row.
        // Upgrade it rather than writing a second assistant row (the front end
        // and the Rust checkpointer both persisted the reply before this, so a
        // supervised turn landed twice). `created_at` is deliberately left as
        // the checkpoint's — the turn was created when generation started, and
        // returning the row's own value keeps this result equal to what the
        // next `get_chat` reads back.
        if role == "assistant" {
            let partial: Option<(i64, String)> = conn
                .query_row(
                    "SELECT id, created_at FROM messages WHERE chat_id = ?1 AND partial = 1 \
                     ORDER BY id DESC LIMIT 1",
                    params![chat_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            if let Some((id, created_at)) = partial {
                conn.execute(
                    "UPDATE messages SET content = ?1, citations = ?2, tool_calls = ?3, \
                     guard = ?4, partial = 0 WHERE id = ?5",
                    params![content, citations_json, tool_calls_json, guard_json, id],
                )
                .map_err(|e| e.to_string())?;

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

                return Ok(MessageInfo {
                    id,
                    role: role.to_string(),
                    content: content.to_string(),
                    citations,
                    tool_calls,
                    created_at,
                    partial: false,
                    guard,
                    confirmed_route: None,
                    confirmed_at: None,
                });
            }
        }

        conn.execute(
            "INSERT INTO messages (chat_id, role, content, citations, tool_calls, created_at, guard)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                chat_id,
                role,
                content,
                citations_json,
                tool_calls_json,
                now,
                guard_json
            ],
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
            // `append_message` writes finished messages only; in-flight turns
            // go through `checkpoint_partial`.
            partial: false,
            guard,
            // A verdict is confirmed later, by the health worker, through
            // `confirm_route` — never at append time.
            confirmed_route: None,
            confirmed_at: None,
        })
    }

    // ---- Supervised triage (P2.1 / P2.5) ------------------------------

    /// The four routes the triage contract defines. `confirm_route` accepts
    /// nothing else, so a typo from the front end is a clean error rather
    /// than a fifth route silently entering the export.
    const ROUTES: [&str; 4] = ["EMERGENCY", "CLINICIAN", "SELF_CARE", "OUT_OF_SCOPE"];

    /// The health worker's decision on a supervised reply. Only assistant
    /// rows carry a route; only the four contract routes are accepted.
    ///
    /// AND ONLY A GUARDED ROW. A confirmation is a decision ABOUT a verdict —
    /// the export's `overridden` column is literally `confirmed_route !=
    /// guard.route` — so a row with no verdict has nothing to agree or
    /// disagree with, and confirming one would write a route that the export
    /// then skips (it only emits guarded replies). The front end already draws
    /// the controls against a guarded message only; this makes the store's
    /// rule the same rule rather than a convention the caller is trusted with.
    ///
    /// Recording a route never rewrites the model's own verdict — that
    /// stays in `guard` — so the export can always show both and say
    /// whether the human overrode the machine.
    ///
    /// Returns the row as it now stands, the same `MessageInfo` shape
    /// [`Self::attach_guard`] returns. The front end needs `confirmed_at`
    /// — the store's own clock, which the caller cannot compute — and
    /// before this returned it the only way to see that value was to
    /// reopen the chat. The read is under the SAME lock acquisition as
    /// the `UPDATE` for `attach_guard`'s reason: two confirmations racing
    /// on one row must not each read back the other's write.
    pub fn confirm_route(
        &self,
        user_id: &str,
        message_id: i64,
        route: &str,
    ) -> Result<MessageInfo, String> {
        if !Self::ROUTES.contains(&route) {
            return Err(format!(
                "unknown route {route}; expected one of {}",
                Self::ROUTES.join(", ")
            ));
        }
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();
        // Read, check and write under ONE lock acquisition, the same shape
        // [`Self::attach_guard`] uses and for the same reason: the verdict this
        // confirmation is about must not be attached (or replaced) between the
        // check and the write.
        let (role, has_guard): (String, bool) = conn
            .query_row(
                "SELECT role, guard IS NOT NULL FROM messages WHERE id = ?1",
                params![message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no assistant message with id {message_id}"))?;
        if role != "assistant" {
            return Err(format!("no assistant message with id {message_id}"));
        }
        if !has_guard {
            return Err(format!(
                "message {message_id} carries no verdict to confirm"
            ));
        }
        conn.execute(
            "UPDATE messages SET confirmed_route = ?1, confirmed_at = ?2 WHERE id = ?3",
            params![route, now_iso(), message_id],
        )
        .map_err(|e| e.to_string())?;
        let sql = format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE id = ?1");
        conn.query_row(&sql, params![message_id], message_from_row)
            .map_err(|e| format!("confirmed route {route}, but row {message_id} read back: {e}"))
    }

    /// Write the guard's verdict onto a row the stream ALREADY persisted.
    ///
    /// This is the mobile path's attachment point, not `append_message`:
    /// `chat_cmds`'s `settle` finalizes the checkpoint row (clearing
    /// `partial`) before the `Done` event that hands the front end the
    /// reply, so by the time the verdict exists there is no in-flight row
    /// left to upgrade — see the module doc comment's "One assistant row
    /// per turn" section. Once Task 6 lands, `Done` will carry the
    /// finalized row's id and the front end will attach the verdict to
    /// THAT row rather than appending a second one; today's front end
    /// still appends, so nothing calls this yet.
    ///
    /// ONE ROW HOLDS BOTH TEXTS. The row `settle` wrote holds the RAW
    /// reply, which is not what the user was shown, so when the verdict
    /// carries a string `displayText` the same `UPDATE` also writes it to
    /// `content`. Nothing is lost: the guard column keeps the raw reply as
    /// `rawReply`, which is where the export reads it from — so reopening
    /// a supervised chat renders the stripped text while the audit trail
    /// still shows what the model actually said. A verdict with no
    /// `displayText` (nothing was stripped, or a caller that does not send
    /// one) leaves `content` exactly as it was.
    ///
    /// Never inserts. Refused, with an error naming the reason, when the
    /// row is not in this account's database, is not an assistant reply,
    /// or already carries a verdict from a DIFFERENT detector build — a
    /// verdict is attached once, and silently replacing one pinned to
    /// another `detectorsSha` would break the audit trail the export
    /// depends on. A repeat carrying the SAME `detectorsSha` is the same
    /// verdict, so it is accepted as a benign retry (a dropped IPC
    /// response, say) and rewrites the same values.
    ///
    /// The confirmation columns are not touched: the health worker's
    /// decision is [`Self::confirm_route`]'s business, never the guard's.
    pub fn attach_guard(
        &self,
        user_id: &str,
        message_id: i64,
        guard: Value,
    ) -> Result<MessageInfo, String> {
        let conn = self.conn_for(user_id)?;
        let conn = conn.lock().unwrap();

        // Read, check and write under ONE lock acquisition, so two attaches
        // racing on the same row cannot both pass the "no verdict yet" check.
        let sql = format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE id = ?1");
        let existing = conn
            .query_row(&sql, params![message_id], message_from_row)
            .optional()
            .map_err(|e| e.to_string())?;
        let Some(mut existing) = existing else {
            // Per-account databases: an id from another account is simply
            // not here, which is this same error rather than a leak.
            return Err(format!("no message with id {message_id} in this account"));
        };
        if existing.role != "assistant" {
            return Err(format!(
                "message {message_id} is a {} turn; only an assistant reply carries a guard verdict",
                existing.role
            ));
        }
        if let Some(prior) = existing.guard.as_ref() {
            let prior_sha = detectors_sha(prior);
            let next_sha = detectors_sha(&guard);
            if prior_sha != next_sha {
                return Err(format!(
                    "message {message_id} already carries a guard verdict from detectors {}; \
                     refusing to replace it with one from {}",
                    prior_sha.unwrap_or("(none)"),
                    next_sha.unwrap_or("(none)")
                ));
            }
        }

        // Owned before `guard` is moved into the returned row below.
        let display_text: Option<String> = guard
            .get("displayText")
            .and_then(Value::as_str)
            .map(str::to_string);

        match display_text.as_deref() {
            Some(text) => conn.execute(
                "UPDATE messages SET guard = ?1, content = ?2 WHERE id = ?3",
                params![guard.to_string(), text, message_id],
            ),
            None => conn.execute(
                "UPDATE messages SET guard = ?1 WHERE id = ?2",
                params![guard.to_string(), message_id],
            ),
        }
        .map_err(|e| e.to_string())?;

        if let Some(text) = display_text {
            existing.content = text;
        }
        existing.guard = Some(guard);
        Ok(existing)
    }

    /// One JSON line per guarded assistant message, paired with the user
    /// turn that preceded it — the triage override log the clinical review
    /// reads. `chat_id: None` exports every chat of this account.
    ///
    /// Takes no lock of its own: every read goes through `get_chat`/
    /// `list_chats`, which each take and release the per-account
    /// connection lock themselves.
    pub fn export_triage_log(&self, user_id: &str, chat_id: Option<i64>) -> Result<String, String> {
        let chats: Vec<ChatInfo> = match chat_id {
            Some(id) => vec![self.get_chat(user_id, id)?.chat],
            None => self.list_chats(user_id)?,
        };
        let mut out = String::new();
        for chat in chats {
            let detail = self.get_chat(user_id, chat.id)?;
            let mut last_user: Option<String> = None;
            for m in detail.messages {
                if m.role == "user" {
                    last_user = Some(m.content.clone());
                    continue;
                }
                // Only a guarded reply is an audit row; an ordinary
                // assistant turn (and any partial left by a kill) is not.
                let Some(guard) = m.guard.as_ref() else {
                    continue;
                };
                let model_route = guard
                    .get("route")
                    .and_then(|v| v.as_str())
                    .unwrap_or("UNCLEAR")
                    .to_string();
                let overridden = m
                    .confirmed_route
                    .as_deref()
                    .map(|c| c != model_route)
                    .unwrap_or(false);
                let line = serde_json::json!({
                    "chat_id": chat.id,
                    "message_id": m.id,
                    "created_at": m.created_at,
                    "user_text": last_user.clone().unwrap_or_default(),
                    "raw_reply": guard.get("rawReply").cloned().unwrap_or(Value::Null),
                    "display_text": m.content,
                    "model_route": model_route,
                    "banner": guard.get("banner").cloned().unwrap_or(Value::Null),
                    "timeframe_stripped": guard.get("timeframeStripped").cloned().unwrap_or(Value::Null),
                    // The heaviest receipt row there is: on a CLINICIAN route
                    // a stated time frame survived every strip above, so the
                    // guard withheld the model's sentences ENTIRELY and showed
                    // its own note instead. `display_text` alone cannot say
                    // that happened — a short note looks like a short answer —
                    // so without this column the review cannot tell a withheld
                    // reply from a terse one.
                    //
                    // A bool, never `null`, for `prohibited_removed`'s reason:
                    // a verdict that omits the key, or carries a non-bool, did
                    // not withhold anything, and that is `false`.
                    "timeframe_unlocated": Value::Bool(
                        guard
                            .get("timeframeUnlocated")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    ),
                    "crisis_line_appended": guard.get("crisisLineAppended").cloned().unwrap_or(Value::Null),
                    "prohibited": guard.get("prohibited").cloned().unwrap_or(Value::Null),
                    // The removal RECEIPT: the sentences the guard actually
                    // took out, verbatim. `prohibited` says a rule fired;
                    // this says what the reader never saw, which is the half
                    // a clinical review cannot reconstruct from anything
                    // else in the row.
                    //
                    // Always a list, never `null`: a `null` here would read
                    // as "unknown", and the two verdicts that produce no
                    // array — one from before the receipt existed, one
                    // carrying a malformed value — both mean "nothing was
                    // removed", which is `[]`.
                    "prohibited_removed": Value::Array(
                        guard
                            .get("prohibitedRemoved")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default(),
                    ),
                    "confirmed_route": m.confirmed_route,
                    "confirmed_at": m.confirmed_at,
                    "overridden": overridden,
                    "detectors_sha": guard.get("detectorsSha").cloned().unwrap_or(Value::Null),
                    "model_id": chat.model_id,
                    "adapter_ids": chat.adapter_ids,
                    // Provenance BY SHA, beside the ids above rather than
                    // instead of them (spec §5 P2.5). An id is only as stable
                    // as the re-pinning discipline behind it: Phase 3 swaps the
                    // adapter under the same `med-triage` id, and a review
                    // reading one of these lines afterwards cannot tell which
                    // bytes produced it. These three can. They are stamped onto
                    // the persisted verdict from the catalog ENTRY the turn was
                    // sent under (see triage-turn.js `guardForPersistence`) —
                    // not from today's catalog, which is the whole point.
                    //
                    // Strings, `""` when absent, never `null`, for
                    // `prohibited_removed`'s reason: the two verdicts that
                    // carry nothing here — one written before this existed, one
                    // from an entry that pins no shas — both mean "not
                    // recorded", and a reader that has to handle `null` as well
                    // as `""` will eventually handle only one of them.
                    "prompt_fingerprint": guard.get("promptFingerprint").and_then(Value::as_str).unwrap_or(""),
                    "model_sha": guard.get("modelSha").and_then(Value::as_str).unwrap_or(""),
                    "adapter_sha": guard.get("adapterSha").and_then(Value::as_str).unwrap_or(""),
                });
                out.push_str(&line.to_string());
                out.push('\n');
            }
        }
        Ok(out)
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

    // ---- Removal (§7 Task 6: "remove account from this device") -------

    /// The chats-side half of an account's optional local-data wipe:
    /// deletes `user_id`'s ENTIRE per-account database file
    /// (`<dir>/<segment>.db`), if any. Sanitized via [`account_dir_segment`]
    /// — the SAME sanitizer [`ConvStore::conn_for`] itself uses to open that
    /// file — so this can only ever target the ONE file `conn_for` would
    /// open for `user_id`; a malformed/traversal id is a hard `Err`, never a
    /// best-effort path. Drops any already-open cached connection for the
    /// account FIRST, under the same `conns` lock `conn_for` uses, so a
    /// stale open handle from earlier in this process is never reused once
    /// the file underneath it is gone. Missing file is not an error
    /// (idempotent — the account may never have created a chat).
    /// `StoreRoot::Memory` (tests only) has nothing on disk to delete —
    /// dropping the cached connection above is the whole story there.
    pub fn delete_account_conversations(&self, user_id: &str) -> Result<(), String> {
        let segment =
            account_dir_segment(user_id).ok_or_else(|| "invalid account id".to_string())?;
        self.conns.lock().unwrap().remove(&segment);
        match &self.root {
            StoreRoot::Dir(dir) => {
                let path = dir.join(format!("{segment}.db"));
                match std::fs::remove_file(&path) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(format!("couldn't delete conversations: {e}")),
                }
            }
            StoreRoot::Memory => Ok(()),
        }
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

/// The detector build a verdict is pinned to, or `None` when it carries no
/// pin. Read defensively (`get`/`as_str`) rather than deserialized: this is
/// the FRONT END's guard JSON exactly as it was stored, and a verdict with
/// no pin must compare equal to another with no pin, not fail.
fn detectors_sha(guard: &Value) -> Option<&str> {
    guard.get("detectorsSha").and_then(Value::as_str)
}

fn message_from_row(row: &rusqlite::Row) -> rusqlite::Result<MessageInfo> {
    let citations_json: Option<String> = row.get(3)?;
    let tool_calls_json: Option<String> = row.get(4)?;
    let guard_json: Option<String> = row.get(7)?;
    Ok(MessageInfo {
        id: row.get(0)?,
        role: row.get(1)?,
        content: row.get(2)?,
        citations: citations_json.and_then(|s| serde_json::from_str(&s).ok()),
        tool_calls: tool_calls_json.and_then(|s| serde_json::from_str(&s).ok()),
        created_at: row.get(5)?,
        partial: row.get::<_, i64>(6)? != 0,
        // Same defensive read as `citations`/`tool_calls`: an unparseable
        // verdict reads as `None` rather than failing the whole chat open.
        guard: guard_json.and_then(|s| serde_json::from_str(&s).ok()),
        confirmed_route: row.get(8)?,
        confirmed_at: row.get(9)?,
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
/// fallback for any other role (there is no separate `"system"`/tool-role
/// message — since Task 7, calc() provenance rides along on the ordinary
/// `"assistant"` row's `tool_calls` column rather than a new role — but
/// deriving the label from the stored value rather than a hardcoded
/// two-arm match means an export never silently drops a future role's
/// turns).
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
/// `serde_json::Value`s here (`None` when the column is `NULL`). Since Task 7,
/// `tool_calls` holds an assistant turn's calc() provenance — `src/app.js`'s
/// `finishStream` sends it as `[{expression, display}]`/`[{expression,
/// error}]` under the `toolCalls` invoke-arg (Tauri's camelCase-arg ->
/// snake_case-param mapping turns that into this struct's `tool_calls`
/// field), and `openChat` reads it back here as `msg.toolCalls`, mirroring
/// how `citations` already round-trips.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageInfo {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub citations: Option<Value>,
    pub tool_calls: Option<Value>,
    pub created_at: String,
    /// True while this row is a generation checkpoint that has not been
    /// finalized — i.e. the turn was still streaming. A row left `true` after a
    /// restart was truncated by the process dying, which is exactly the
    /// distinction §11's kill-restore test asserts.
    ///
    /// Skipped when false, so payloads for ordinary completed messages are
    /// byte-identical to what they were before this column existed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
    /// The product guard's verdict on a supervised reply (`src/triage/
    /// guard.js`): route, banner, the raw reply, what was stripped, the
    /// detector pin. `None` for every ordinary message — and skipped when
    /// `None`, so an unguarded message's payload stays byte-identical to
    /// what it was before this column existed (same rule as `partial`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard: Option<Value>,
    /// The health worker's confirmed route, once chosen; `overridden` in the
    /// export is `confirmed_route != guard.route`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_route: Option<String>,
    /// When that confirmation was recorded (`now_iso()`), set by
    /// [`ConvStore::confirm_route`] alongside `confirmed_route`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_at: Option<String>,
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
pub(crate) fn current_user_id(app: &AppHandle) -> Option<String> {
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

/// §7 S7-5: called by `src/app.js`'s `maybeAutoTitle` after the first
/// complete exchange in a new chat, with a model-generated title. Returns
/// `false` (not an error) when nothing was applied — see
/// [`ConvStore::auto_title_chat`] for the full guard.
#[tauri::command]
pub async fn auto_title_chat(id: i64, title: String, app: AppHandle) -> Result<bool, String> {
    let user_id = current_user_id(&app).ok_or_else(sign_in_required)?;
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ConvStore>()
            .auto_title_chat(&user_id, id, &title)
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
    guard: Option<Value>,
    app: AppHandle,
) -> Result<MessageInfo, String> {
    let user_id = current_user_id(&app).ok_or_else(sign_in_required)?;
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ConvStore>().append_message(
            &user_id, chat_id, &role, &content, citations, tool_calls, guard,
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

/// Records the health worker's route for one supervised reply. `route` is
/// a plain string over IPC (the four contract routes), validated in
/// [`ConvStore::confirm_route`] — an unknown one is a clean error, never a
/// stored value.
///
/// Resolves with the updated row, so the confirmation banner can show the
/// store's own `confirmedAt` without reopening the chat. `src/triage-
/// confirm.js`'s `confirmResult` already prefers a returned row over the
/// route it asked for, so nothing on the front end had to learn a new shape.
#[tauri::command]
pub async fn confirm_route(
    message_id: i64,
    route: String,
    app: AppHandle,
) -> Result<MessageInfo, String> {
    let user_id = current_user_id(&app).ok_or_else(sign_in_required)?;
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ConvStore>()
            .confirm_route(&user_id, message_id, &route)
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

/// Attaches the guard's verdict — and the text the user was actually shown
/// — to the assistant row the stream already persisted. The mobile path's
/// attachment point; the row id will come from `ChatEvent::Done` once Task
/// 6 makes that event carry it. Async + `spawn_blocking` like every other
/// command in this module: SQLite I/O must not run on the IPC thread.
#[tauri::command]
pub async fn attach_guard(
    message_id: i64,
    guard: Value,
    app: AppHandle,
) -> Result<MessageInfo, String> {
    let user_id = current_user_id(&app).ok_or_else(sign_in_required)?;
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ConvStore>()
            .attach_guard(&user_id, message_id, guard)
    })
    .await
    .map_err(|_| JOIN_ERROR_MESSAGE.to_string())?
}

/// Writes the triage override log (JSONL) to `path`, the user's own OS save
/// choice — the same write shape as `export_chat_to_file`, and the same
/// reasoning for one command that both formats AND writes: the only file
/// this can write is `path`, and the only content is `export_triage_log`'s
/// own deterministic output. `chat_id: None` exports every chat of the
/// signed-in account.
#[tauri::command]
pub async fn export_triage_log_to_file(
    chat_id: Option<i64>,
    path: String,
    app: AppHandle,
) -> Result<(), String> {
    let user_id = current_user_id(&app).ok_or_else(|| "Sign in to export.".to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let content = app
            .state::<ConvStore>()
            .export_triage_log(&user_id, chat_id)?;
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

    // ---- Partial-turn checkpointing (kill-restore gate) --------------

    /// The distinction the whole design exists for: after a simulated kill, a
    /// checkpointed turn is still readable AND still identifiable as truncated.
    /// An in-place update of an ordinary row would satisfy the first half and
    /// silently fail the second.
    #[test]
    fn partial_survives_a_kill_and_stays_marked_truncated() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "c", None, None, "hero", vec![])
            .unwrap();
        store
            .append_message(USER, chat.id, "user", "2+2?", None, None, None)
            .unwrap();

        // Generation streams; each checkpoint advances the same row.
        let id = store.checkpoint_partial(USER, chat.id, "The ans").unwrap();
        let again = store.checkpoint_partial(USER, chat.id, "The answer is").unwrap();
        assert_eq!(id, again, "checkpointing must advance one row, not accumulate");

        // ...and then the process dies. Nothing finalizes.
        let msgs = store.get_chat(USER, chat.id).unwrap().messages;
        assert_eq!(msgs.len(), 2, "the partial turn must be readable after a kill");
        let last = msgs.last().unwrap();
        assert_eq!(last.role, "assistant");
        assert_eq!(last.content, "The answer is", "truncated content must be intact");
        assert!(last.partial, "a killed turn must remain identifiable as truncated");
    }

    /// Stop before the first token: the store half of `chat_cmds::settle`'s
    /// empty-outcome branch.
    ///
    /// A checkpoint row exists (the checkpointer had fired, or the cancel
    /// arrived after one), the turn returns `Ok` with no text, and the row is
    /// DISCARDED rather than finalized. Finalizing left a blank, non-partial
    /// assistant row; the front end persists nothing for an empty reply, so
    /// that row carried no verdict and `replayMessage` withheld it behind
    /// "Unverified reply" — a safety notice about a reply that never existed.
    ///
    /// `settle` itself is not reachable from this harness: it is a private fn
    /// inside `#[cfg(mobile)] mod imp` and takes an `AppHandle` with managed
    /// state. Its branch is compiled by the Android cross-check
    /// (`cargo check -p cleophis --target aarch64-linux-android --tests`); what
    /// is asserted here is the store behaviour that branch depends on.
    #[test]
    fn a_checkpointed_turn_that_produced_nothing_leaves_no_row_to_withhold() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "Triage", None, None, "med-triage", vec![])
            .unwrap();
        store
            .append_message(USER, chat.id, "user", "sore throat", None, None, None)
            .unwrap();
        store.checkpoint_partial(USER, chat.id, "").unwrap();
        assert_eq!(
            store.get_chat(USER, chat.id).unwrap().messages.len(),
            2,
            "the checkpoint row is there to be discarded"
        );

        store.discard_partial(USER, chat.id).unwrap();

        let msgs = store.get_chat(USER, chat.id).unwrap().messages;
        assert_eq!(
            msgs.len(),
            1,
            "Stop before the first token must leave no reply"
        );
        assert_eq!(msgs[0].role, "user");
        // And nothing for the export to read either: an unguarded assistant
        // row is skipped there, but it is not skipped on screen.
        assert_eq!(store.export_triage_log(USER, Some(chat.id)).unwrap(), "");
    }

    #[test]
    fn finalizing_clears_the_marker_and_a_short_answer_is_not_mistaken_for_truncated() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "c", None, None, "hero", vec![])
            .unwrap();

        let id = store.checkpoint_partial(USER, chat.id, "4").unwrap();
        store
            .finalize_partial(USER, id, "4", None, Some(json!([{ "expression": "2+2" }])))
            .unwrap();

        let msgs = store.get_chat(USER, chat.id).unwrap().messages;
        assert_eq!(msgs.len(), 1);
        let m = &msgs[0];
        // "4" is as short as a truncated turn would be; only the marker
        // distinguishes them, which is the point.
        assert_eq!(m.content, "4");
        assert!(!m.partial, "a finalized turn must not read as truncated");
        assert!(m.tool_calls.is_some(), "finalize must persist tool calls");
    }

    #[test]
    fn a_new_turn_after_recovery_does_not_reuse_the_dead_partial_row() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "c", None, None, "hero", vec![])
            .unwrap();
        let dead = store.checkpoint_partial(USER, chat.id, "half").unwrap();
        store.finalize_partial(USER, dead, "half", None, None).unwrap();

        let fresh = store.checkpoint_partial(USER, chat.id, "new turn").unwrap();
        assert_ne!(fresh, dead, "a finalized row must not be re-used as the next checkpoint");
    }

    #[test]
    fn discarding_removes_only_the_partial_row() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "c", None, None, "hero", vec![])
            .unwrap();
        store
            .append_message(USER, chat.id, "user", "keep me", None, None, None)
            .unwrap();
        store.checkpoint_partial(USER, chat.id, "throw me away").unwrap();

        store.discard_partial(USER, chat.id).unwrap();

        let msgs = store.get_chat(USER, chat.id).unwrap().messages;
        assert_eq!(msgs.len(), 1, "discard must not touch completed messages");
        assert_eq!(msgs[0].content, "keep me");
    }

    /// Ordinary appends are unaffected — the desktop path must not acquire a
    /// truncated-looking message just because the column now exists.
    #[test]
    fn ordinary_appends_are_never_marked_partial() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "c", None, None, "hero", vec![])
            .unwrap();
        let m = store
            .append_message(USER, chat.id, "assistant", "done", None, None, None)
            .unwrap();
        assert!(!m.partial);
        assert!(!store.get_chat(USER, chat.id).unwrap().messages[0].partial);
    }

    // ---- Supervised triage (P2.1 / P2.5) -----------------------------

    /// The verdict round-trips through the `guard` column, and an ORDINARY
    /// message's IPC payload is unchanged — no `guard: null` appears on the
    /// tutor hero's rows just because the column now exists (the same
    /// `skip_serializing_if` rule `partial` already follows).
    #[test]
    fn guard_round_trips_and_an_unguarded_message_serialises_as_before() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "Triage", None, None, "med-triage", vec![])
            .unwrap();
        let guard = json!({
            "route": "CLINICIAN",
            "banner": "clinician",
            "rawReply": "See your GP today.",
            "timeframeStripped": ["today"]
        });
        let plain = store
            .append_message(USER, chat.id, "user", "hello", None, None, None)
            .unwrap();
        let guarded = store
            .append_message(
                USER,
                chat.id,
                "assistant",
                "See your GP.",
                None,
                None,
                Some(guard.clone()),
            )
            .unwrap();
        assert_eq!(guarded.guard, Some(guard));
        assert_eq!(guarded.confirmed_route, None);

        let v = serde_json::to_value(&plain).unwrap();
        assert!(
            v.get("guard").is_none(),
            "an unguarded message must not grow a null field: {v}"
        );

        let back = store.get_chat(USER, chat.id).unwrap().messages;
        assert_eq!(back[1].guard.as_ref().unwrap()["route"], "CLINICIAN");
    }

    #[test]
    fn append_message_upgrades_a_partial_row_instead_of_writing_a_second_assistant_row() {
        // On mobile the FE and the Rust checkpointer both persisted the reply
        // (two assistant rows per turn). The FE's append must land on the
        // partial row the checkpointer already holds.
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "Triage", None, None, "med-triage", vec![])
            .unwrap();
        store
            .append_message(USER, chat.id, "user", "q", None, None, None)
            .unwrap();
        let partial_id = store
            .checkpoint_partial(USER, chat.id, "See your GP tod")
            .unwrap();
        let info = store
            .append_message(
                USER,
                chat.id,
                "assistant",
                "See your GP.",
                None,
                None,
                Some(json!({"route": "CLINICIAN"})),
            )
            .unwrap();
        assert_eq!(
            info.id, partial_id,
            "the partial row was upgraded, not duplicated"
        );

        let msgs = store.get_chat(USER, chat.id).unwrap().messages;
        assert_eq!(msgs.iter().filter(|m| m.role == "assistant").count(), 1);
        assert_eq!(msgs[1].content, "See your GP.");
        assert!(!msgs[1].partial);
        assert_eq!(msgs[1].guard.as_ref().unwrap()["route"], "CLINICIAN");
    }

    /// A USER append must never be diverted onto the in-flight assistant
    /// row — the upgrade is an assistant-only path.
    #[test]
    fn a_user_append_never_upgrades_the_in_flight_assistant_row() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "Triage", None, None, "med-triage", vec![])
            .unwrap();
        let partial_id = store.checkpoint_partial(USER, chat.id, "half").unwrap();
        let user = store
            .append_message(USER, chat.id, "user", "next question", None, None, None)
            .unwrap();
        assert_ne!(user.id, partial_id);

        let msgs = store.get_chat(USER, chat.id).unwrap().messages;
        assert_eq!(msgs.len(), 2);
        assert!(msgs.iter().any(|m| m.role == "assistant" && m.partial));
    }

    /// The mobile attachment point: the stream has already persisted and
    /// FINALIZED the reply, so the verdict is written onto that row — no
    /// second assistant row appears.
    #[test]
    fn attach_guard_writes_the_verdict_onto_a_finalized_row_and_creates_none() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "Triage", None, None, "med-triage", vec![])
            .unwrap();
        store
            .append_message(USER, chat.id, "user", "chest pain", None, None, None)
            .unwrap();
        // Exactly what `chat_cmds`'s stream does: checkpoint, then finalize
        // BEFORE the front end ever hears about the turn.
        let row = store
            .checkpoint_partial(USER, chat.id, "Call 999 no")
            .unwrap();
        store
            .finalize_partial(USER, row, "Call 999 now.", None, None)
            .unwrap();
        let before = store.get_chat(USER, chat.id).unwrap().messages.len();

        let updated = store
            .attach_guard(
                USER,
                row,
                json!({"route": "EMERGENCY", "detectorsSha": "abc"}),
            )
            .unwrap();
        assert_eq!(updated.id, row);
        assert_eq!(updated.guard.as_ref().unwrap()["route"], "EMERGENCY");
        assert!(
            !updated.partial,
            "attaching must not re-open a finished row"
        );

        let msgs = store.get_chat(USER, chat.id).unwrap().messages;
        assert_eq!(msgs.len(), before, "attaching must never insert a row");
        assert_eq!(msgs.iter().filter(|m| m.role == "assistant").count(), 1);
        assert_eq!(msgs[1].guard.as_ref().unwrap()["detectorsSha"], "abc");
        assert_eq!(msgs[1].content, "Call 999 now.");
    }

    /// One row, both texts: the row `settle` wrote holds the RAW reply, so
    /// attaching replaces `content` with what the user was actually shown
    /// while the verdict keeps the raw reply for the audit trail.
    #[test]
    fn attach_guard_replaces_the_raw_content_with_the_guarded_display_text() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "Triage", None, None, "med-triage", vec![])
            .unwrap();
        store
            .append_message(USER, chat.id, "user", "chest pain", None, None, None)
            .unwrap();
        let row = store
            .checkpoint_partial(USER, chat.id, "Call 999 within")
            .unwrap();
        // What the model said, timeframe and all — this is what the stream
        // persists, and it is NOT what the user was shown.
        store
            .finalize_partial(USER, row, "Call 999 within 10 minutes.", None, None)
            .unwrap();

        let updated = store
            .attach_guard(
                USER,
                row,
                json!({
                    "route": "EMERGENCY",
                    "rawReply": "Call 999 within 10 minutes.",
                    "displayText": "Call 999 now.",
                    "timeframeStripped": ["within 10 minutes"],
                    "detectorsSha": "abc"
                }),
            )
            .unwrap();
        assert_eq!(updated.content, "Call 999 now.");

        let back = store.get_chat(USER, chat.id).unwrap().messages;
        assert_eq!(back[1].content, "Call 999 now.", "the row renders stripped");
        assert_eq!(
            back[1].guard.as_ref().unwrap()["rawReply"],
            "Call 999 within 10 minutes.",
            "the raw reply survives inside the verdict"
        );

        // ...and the export shows both, not the same string twice.
        let log = store.export_triage_log(USER, Some(chat.id)).unwrap();
        let lines: Vec<serde_json::Value> = log
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["display_text"], "Call 999 now.");
        assert_eq!(lines[0]["raw_reply"], "Call 999 within 10 minutes.");
        assert_ne!(
            lines[0]["display_text"], lines[0]["raw_reply"],
            "a stripped reply must not export as if nothing was stripped"
        );
    }

    /// Nothing stripped, nothing to rewrite: a verdict with no
    /// `displayText` leaves the row's content alone.
    #[test]
    fn attach_guard_without_display_text_leaves_the_content_untouched() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "Triage", None, None, "med-triage", vec![])
            .unwrap();
        let m = store
            .append_message(
                USER,
                chat.id,
                "assistant",
                "Rest and fluids.",
                None,
                None,
                None,
            )
            .unwrap();

        let updated = store
            .attach_guard(
                USER,
                m.id,
                json!({"route": "SELF_CARE", "detectorsSha": "abc"}),
            )
            .unwrap();
        assert_eq!(updated.content, "Rest and fluids.");
        let back = store.get_chat(USER, chat.id).unwrap().messages;
        assert_eq!(back[0].content, "Rest and fluids.");
        assert_eq!(back[0].guard.as_ref().unwrap()["route"], "SELF_CARE");
    }

    /// A verdict is attached ONCE. A repeat from the same detector build is
    /// a benign retry; one from a different build is refused, because
    /// silently replacing it would break the export's audit trail.
    #[test]
    fn attach_guard_is_idempotent_for_one_sha_and_refuses_a_different_one() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "Triage", None, None, "med-triage", vec![])
            .unwrap();
        let m = store
            .append_message(
                USER,
                chat.id,
                "assistant",
                "Call 999 now.",
                None,
                None,
                None,
            )
            .unwrap();

        store
            .attach_guard(
                USER,
                m.id,
                json!({"route": "EMERGENCY", "detectorsSha": "abc"}),
            )
            .unwrap();
        let again = store
            .attach_guard(
                USER,
                m.id,
                json!({"route": "EMERGENCY", "detectorsSha": "abc"}),
            )
            .unwrap();
        assert_eq!(again.guard.as_ref().unwrap()["route"], "EMERGENCY");

        let err = store
            .attach_guard(
                USER,
                m.id,
                json!({"route": "SELF_CARE", "detectorsSha": "def"}),
            )
            .unwrap_err();
        assert!(err.contains("abc") && err.contains("def"), "{err}");
        // ...and the stored verdict is the FIRST one, untouched.
        let back = store.get_chat(USER, chat.id).unwrap().messages;
        assert_eq!(back[0].guard.as_ref().unwrap()["route"], "EMERGENCY");
        assert_eq!(back[0].guard.as_ref().unwrap()["detectorsSha"], "abc");
    }

    #[test]
    fn attach_guard_refuses_a_user_turn_and_an_id_this_account_does_not_have() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "Triage", None, None, "med-triage", vec![])
            .unwrap();
        let u = store
            .append_message(USER, chat.id, "user", "chest pain", None, None, None)
            .unwrap();

        let err = store
            .attach_guard(USER, u.id, json!({"route": "EMERGENCY"}))
            .unwrap_err();
        assert!(err.contains("user"), "{err}");
        assert!(store.get_chat(USER, chat.id).unwrap().messages[0]
            .guard
            .is_none());

        // Per-account databases: another account's row id is simply not in
        // this account's database, and attaching to it is a clean error.
        let err = store
            .attach_guard("acct-b", u.id, json!({"route": "EMERGENCY"}))
            .unwrap_err();
        assert!(err.contains(&u.id.to_string()), "{err}");
    }

    #[test]
    fn confirm_route_records_the_override_and_rejects_an_unknown_route() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "Triage", None, None, "med-triage", vec![])
            .unwrap();
        let m = store
            .append_message(
                USER,
                chat.id,
                "assistant",
                "See your GP.",
                None,
                None,
                Some(json!({"route": "CLINICIAN"})),
            )
            .unwrap();

        // The returned row IS the row: the front end reflects the
        // confirmation from this value alone, without reopening the chat.
        let info = store.confirm_route(USER, m.id, "EMERGENCY").unwrap();
        assert_eq!(info.id, m.id);
        assert_eq!(info.confirmed_route.as_deref(), Some("EMERGENCY"));
        assert!(info.confirmed_at.is_some());
        assert_eq!(info.guard.as_ref().unwrap()["route"], "CLINICIAN");

        let back = store.get_chat(USER, chat.id).unwrap().messages;
        assert_eq!(back[0].confirmed_route.as_deref(), Some("EMERGENCY"));
        assert!(back[0].confirmed_at.is_some());
        // The model's own verdict is never rewritten by the confirmation.
        assert_eq!(back[0].guard.as_ref().unwrap()["route"], "CLINICIAN");
        // ...and what came back is what was stored, not a hopeful echo of
        // the request: a reopen must agree with it field for field.
        assert_eq!(back[0], info);

        let err = store.confirm_route(USER, m.id, "MAYBE").unwrap_err();
        assert!(err.contains("route"), "{err}");
    }

    /// A route can only be confirmed on an assistant row, and a bad id is a
    /// named error rather than a silent no-op.
    #[test]
    fn confirm_route_rejects_a_message_that_is_not_an_assistant_reply() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "Triage", None, None, "med-triage", vec![])
            .unwrap();
        let u = store
            .append_message(USER, chat.id, "user", "chest pain", None, None, None)
            .unwrap();
        let err = store.confirm_route(USER, u.id, "EMERGENCY").unwrap_err();
        assert!(err.contains(&u.id.to_string()), "{err}");
    }

    #[test]
    fn export_triage_log_pairs_each_verdict_with_the_user_turn_before_it() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(
                USER,
                "Triage",
                None,
                None,
                "med-triage",
                vec!["triage-armb-v8".into()],
            )
            .unwrap();
        store
            .append_message(
                USER,
                chat.id,
                "user",
                "chest pain down my arm",
                None,
                None,
                None,
            )
            .unwrap();
        let a = store
            .append_message(
                USER,
                chat.id,
                "assistant",
                "Call 999 now.",
                None,
                None,
                Some(json!({
                    "route": "EMERGENCY",
                    "banner": "emergency",
                    "rawReply": "Call 999 now. Take 300mg aspirin.",
                    "prohibited": ["dosage"],
                    "prohibitedRemoved": ["Take 300mg aspirin."],
                    "detectorsSha": "abc",
                    // Stamped at persistence by `guardForPersistence`, not by
                    // `applyGuard` — see the export's provenance block.
                    "promptFingerprint": "67b7f1633f30",
                    "modelSha": "25162bff",
                    "adapterSha": "5304e464"
                })),
            )
            .unwrap();
        store.confirm_route(USER, a.id, "EMERGENCY").unwrap();

        // A SECOND guarded turn in the same chat, on the one route that can
        // produce `timeframeUnlocated` — CLINICIAN. Two audit rows also make
        // the user-turn pairing a real assertion rather than a tautology: with
        // one pair, a bug that always reports the FIRST user turn passes.
        store
            .append_message(
                USER,
                chat.id,
                "user",
                "when should I see someone?",
                None,
                None,
                None,
            )
            .unwrap();
        let b = store
            .append_message(
                USER,
                chat.id,
                "assistant",
                "Ask a clinician about the timing.",
                None,
                None,
                Some(json!({
                    "route": "CLINICIAN",
                    "banner": "clinician",
                    "rawReply": "See a GP within 48 hours.",
                    "timeframeStripped": [],
                    "timeframeUnlocated": true,
                    "detectorsSha": "abc"
                })),
            )
            .unwrap();
        store.confirm_route(USER, b.id, "CLINICIAN").unwrap();

        let log = store.export_triage_log(USER, Some(chat.id)).unwrap();
        let lines: Vec<serde_json::Value> = log
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["user_text"], "chest pain down my arm");
        assert_eq!(lines[0]["model_route"], "EMERGENCY");
        assert_eq!(lines[0]["confirmed_route"], "EMERGENCY");
        assert_eq!(lines[0]["overridden"], false);
        assert_eq!(lines[0]["adapter_ids"][0], "triage-armb-v8");
        assert_eq!(lines[0]["detectors_sha"], "abc");
        // Spec §5 P2.5's provenance triple, BESIDE the ids rather than instead
        // of them: `model_id`/`adapter_ids` above still say which entry, these
        // say which bytes. Phase 3 re-points the adapter under the same id, so
        // only these three can date a line after that.
        assert_eq!(lines[0]["model_id"], "med-triage");
        assert_eq!(lines[0]["prompt_fingerprint"], "67b7f1633f30");
        assert_eq!(lines[0]["model_sha"], "25162bff");
        assert_eq!(lines[0]["adapter_sha"], "5304e464");
        // The receipt: what the reader never saw, quoted as it was written.
        // The raw reply still holds it, but only this column says which
        // sentences the guard is claiming to have taken out.
        assert_eq!(lines[0]["prohibited"][0], "dosage");
        assert_eq!(
            lines[0]["prohibited_removed"],
            json!(["Take 300mg aspirin."])
        );
        // Absent from this verdict, so `false` — a bool, not null.
        assert_eq!(lines[0]["timeframe_unlocated"], json!(false));

        // The second row: paired with the SECOND user turn, and carrying the
        // flag that says the reader was shown none of the model's sentences.
        // Every receipt row the UI renders is now readable from the export.
        assert_eq!(lines[1]["user_text"], "when should I see someone?");
        assert_eq!(lines[1]["model_route"], "CLINICIAN");
        assert_eq!(lines[1]["timeframe_unlocated"], json!(true));
        assert_eq!(lines[1]["raw_reply"], "See a GP within 48 hours.");
        assert_eq!(
            lines[1]["display_text"],
            "Ask a clinician about the timing."
        );
        assert_eq!(lines[1]["prohibited_removed"], json!([]));
        assert_eq!(lines[1]["overridden"], false);
        // This verdict pins no provenance — a row from before the stamp
        // existed, or an entry that names no shas. Empty STRINGS, never null,
        // so a reader has one absent-value to handle rather than two.
        assert_eq!(lines[1]["prompt_fingerprint"], "");
        assert_eq!(lines[1]["model_sha"], "");
        assert_eq!(lines[1]["adapter_sha"], "");
    }

    /// A confirmation is a decision ABOUT a verdict, so a row that carries
    /// none cannot be confirmed: the export's `overridden` column is
    /// `confirmed_route != guard.route`, and the export emits guarded
    /// replies only — a route written onto an unguarded row would be a
    /// decision recorded nowhere anyone reads.
    #[test]
    fn confirm_route_refuses_a_row_that_carries_no_verdict() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "Triage", None, None, "med-triage", vec![])
            .unwrap();
        // An assistant row with no guard — what `settle` leaves behind when
        // `attach_guard` never lands.
        let bare = store
            .append_message(USER, chat.id, "assistant", "See your GP.", None, None, None)
            .unwrap();
        let err = store.confirm_route(USER, bare.id, "EMERGENCY").unwrap_err();
        assert!(err.contains(&bare.id.to_string()), "{err}");
        assert!(err.contains("no verdict"), "{err}");

        // Refused means NOTHING was written, not "written and reported".
        let back = store.get_chat(USER, chat.id).unwrap().messages;
        assert_eq!(back[0].confirmed_route, None);
        assert_eq!(back[0].confirmed_at, None);

        // And the same row, once a verdict is attached, confirms normally —
        // so the refusal is about the missing verdict and not about the row.
        store
            .attach_guard(
                USER,
                bare.id,
                json!({"route": "CLINICIAN", "detectorsSha": "abc"}),
            )
            .unwrap();
        let info = store.confirm_route(USER, bare.id, "EMERGENCY").unwrap();
        assert_eq!(info.confirmed_route.as_deref(), Some("EMERGENCY"));
    }

    /// An UNGUARDED chat contributes no lines, and a confirmation that
    /// differs from the model's route is reported as an override.
    #[test]
    fn export_triage_log_skips_unguarded_turns_and_flags_a_real_override() {
        let store = ConvStore::new_in_memory();
        let plain = store
            .create_chat(USER, "Tutor", None, None, "socratic-tutor", vec![])
            .unwrap();
        store
            .append_message(USER, plain.id, "user", "teach me", None, None, None)
            .unwrap();
        store
            .append_message(USER, plain.id, "assistant", "sure", None, None, None)
            .unwrap();
        assert_eq!(store.export_triage_log(USER, Some(plain.id)).unwrap(), "");

        let chat = store
            .create_chat(USER, "Triage", None, None, "med-triage", vec![])
            .unwrap();
        store
            .append_message(USER, chat.id, "user", "sore throat", None, None, None)
            .unwrap();
        let a = store
            .append_message(
                USER,
                chat.id,
                "assistant",
                "Rest and fluids.",
                None,
                None,
                Some(json!({"route": "SELF_CARE", "banner": "self-care"})),
            )
            .unwrap();
        store.confirm_route(USER, a.id, "CLINICIAN").unwrap();

        // Every chat of the account, not just one.
        let log = store.export_triage_log(USER, None).unwrap();
        let lines: Vec<serde_json::Value> = log
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 1, "only the guarded turn is an audit row");
        assert_eq!(lines[0]["user_text"], "sore throat");
        assert_eq!(lines[0]["model_route"], "SELF_CARE");
        assert_eq!(lines[0]["confirmed_route"], "CLINICIAN");
        assert_eq!(lines[0]["overridden"], true);
        assert_eq!(lines[0]["detectors_sha"], serde_json::Value::Null);
        // A verdict carrying no removals reports an EMPTY LIST, not null —
        // the review reads this as "nothing was removed", and a null would
        // read as "not known". Note `detectors_sha` above is deliberately
        // still null: an absent pin genuinely IS unknown.
        assert_eq!(lines[0]["prohibited_removed"], json!([]));
        assert_eq!(lines[0]["timeframe_unlocated"], json!(false));
    }

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
            .append_message(USER, chat.id, "user", "hello", None, None, None)
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

    /// Task 7: `append_message`'s `tool_calls` arg (calc() provenance —
    /// `[{expression, display}]`/`[{expression, error}]` from `src/app.js`'s
    /// `finishStream`) round-trips through `get_chat` intact — mirrors t1's
    /// citations round-trip, but for the `tool_calls` column instead.
    /// Exercises both shapes (a clean eval alongside a domain error) in one
    /// message, plus a sibling message with no tool_calls at all, so the
    /// `NULL` column -> `None` path stays covered too.
    #[test]
    fn t1b_append_message_tool_calls_round_trips_through_get_chat() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "Calc chat", None, None, "hero-llama", vec![])
            .unwrap();

        let m1 = store
            .append_message(
                USER,
                chat.id,
                "user",
                "what's 2+2 and 1/0?",
                None,
                None,
                None,
            )
            .unwrap();
        let tool_calls = json!([
            {"expression": "2+2", "display": "4"},
            {"expression": "1/0", "error": "division by zero"},
        ]);
        let m2 = store
            .append_message(
                USER,
                chat.id,
                "assistant",
                "2+2 is 4; 1/0 is undefined.",
                None,
                Some(tool_calls.clone()),
                None,
            )
            .unwrap();

        let detail = store.get_chat(USER, chat.id).unwrap();
        assert_eq!(detail.messages.len(), 2);
        assert_eq!(detail.messages[0].id, m1.id);
        assert!(detail.messages[0].tool_calls.is_none());
        assert_eq!(detail.messages[1].id, m2.id);
        assert_eq!(detail.messages[1].tool_calls, Some(tool_calls));
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
            .append_message(USER, chat.id, "user", "unique_marker_xyz", None, None, None)
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
            .append_message(USER, chat.id, "user", "first", None, None, None)
            .unwrap();
        let after_first = store.get_chat(USER, chat.id).unwrap().chat;
        assert!(after_first.updated_at > chat.updated_at);

        std::thread::sleep(std::time::Duration::from_millis(20));
        store
            .append_message(USER, chat.id, "assistant", "second", None, None, None)
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
    /// (pre-model_id/adapter_ids) `chats` shape AND the old (pre-partial,
    /// pre-guard) `messages` shape — both hand-built directly on disk here,
    /// bypassing `SCHEMA_SQL` entirely — still opens cleanly the first time
    /// `ConvStore::conn_for` touches it, and `create_chat`/`get_chat`/
    /// `append_message` against it succeed with the new columns backfilled
    /// to their defaults rather than erroring on the missing columns. This
    /// exercises the REAL open path (`ConvStore::new` + a real directory),
    /// not just `migrate_chats_columns`/`migrate_messages_columns` in
    /// isolation.
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
                // `folders` too, because the bundled SQLite has
                // `SQLITE_DEFAULT_FOREIGN_KEYS` on: `chats.folder_id`'s
                // parent table has to exist before a row can be inserted.
                "CREATE TABLE folders (
                    id INTEGER PRIMARY KEY, name TEXT NOT NULL,
                    sort_order INTEGER NOT NULL DEFAULT 0
                );
                CREATE TABLE chats (
                    id INTEGER PRIMARY KEY, folder_id INTEGER REFERENCES folders(id),
                    title TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
                    pinned INTEGER NOT NULL DEFAULT 0, archived INTEGER NOT NULL DEFAULT 0,
                    mounted_packs TEXT
                );
                CREATE TABLE messages (
                    id INTEGER PRIMARY KEY,
                    chat_id INTEGER NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
                    role TEXT NOT NULL, content TEXT NOT NULL,
                    citations TEXT,
                    tool_calls TEXT,
                    created_at TEXT NOT NULL
                );
                INSERT INTO chats (id, title, created_at, updated_at, pinned, archived)
                    VALUES (1, 'Old chat', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 0, 0);
                INSERT INTO messages (chat_id, role, content, created_at)
                    VALUES (1, 'assistant', 'written before the guard', '2026-01-01T00:00:01Z');",
            )
            .unwrap();
        }

        let store = ConvStore::new(dir.clone());
        let chat = store
            .create_chat(USER, "Migrated", None, None, "hero-llama", vec![])
            .unwrap();
        assert_eq!(chat.model_id, "hero-llama");
        assert!(chat.adapter_ids.is_empty());

        // A `messages` row written before `partial`/`guard`/
        // `confirmed_route`/`confirmed_at` existed still reads back: the
        // migration backfilled it to no verdict and no confirmation, which
        // is exactly what a pre-guard message carries.
        let old = store.get_chat(USER, 1).unwrap();
        assert_eq!(old.chat.title, "Old chat");
        assert_eq!(old.messages.len(), 1);
        assert_eq!(old.messages[0].content, "written before the guard");
        assert!(!old.messages[0].partial);
        assert!(old.messages[0].guard.is_none());
        assert!(old.messages[0].confirmed_route.is_none());
        assert!(old.messages[0].confirmed_at.is_none());

        // ...and the migrated table takes a guarded write straight away.
        let m = store
            .append_message(
                USER,
                1,
                "assistant",
                "Call 999 now.",
                None,
                None,
                Some(json!({"route": "EMERGENCY"})),
            )
            .unwrap();
        store.confirm_route(USER, m.id, "EMERGENCY").unwrap();
        let after = store.get_chat(USER, 1).unwrap().messages;
        assert_eq!(after[1].guard.as_ref().unwrap()["route"], "EMERGENCY");
        assert_eq!(after[1].confirmed_route.as_deref(), Some("EMERGENCY"));

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
            .append_message(
                USER,
                chat.id,
                "user",
                "What is vitamin K?",
                None,
                None,
                None,
            )
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
            .append_message(USER, chat.id, "user", "hi", None, None, None)
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
            .append_message(USER, chat.id, "user", "hi", None, None, None)
            .unwrap();
        let citations = json!([{"n": 1, "docTitle": "Doc", "locator": "p.1"}]);
        store
            .append_message(
                USER,
                chat.id,
                "assistant",
                "hello",
                Some(citations),
                None,
                None,
            )
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
            .append_message(USER, chat.id, "user", "one", None, None, None)
            .unwrap();
        store
            .append_message(USER, chat.id, "assistant", "two", None, None, None)
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

    /// Reads `chats.title_auto` directly for a given chat. There's no
    /// public getter for it — the FE never sees this column, only its
    /// effect (see the module doc comment's "Auto-title" section) — so the
    /// tests below that assert on it go straight at the per-account
    /// connection, the same way `t8`/`t21` build a raw pre-migration
    /// `chats` table.
    fn title_auto_of(store: &ConvStore, user_id: &str, id: i64) -> i64 {
        let conn = store.conn_for(user_id).unwrap();
        let conn = conn.lock().unwrap();
        conn.query_row(
            "SELECT title_auto FROM chats WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .unwrap()
    }

    /// create_chat: a new chat's `title_auto` starts at 1 (eligible for
    /// auto-titling) — via `SCHEMA_SQL`'s `DEFAULT 1`, not an explicit
    /// INSERT column.
    #[test]
    fn t16_create_chat_starts_title_auto_1() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "New chat", None, None, "hero-llama", vec![])
            .unwrap();
        assert_eq!(title_auto_of(&store, USER, chat.id), 1);
    }

    /// auto_title_chat on a fresh (never-renamed) chat applies the new
    /// title, and `chat_fts` stays in sync — a search for a word in the
    /// NEW title finds the chat.
    #[test]
    fn t17_auto_title_chat_applies_on_a_fresh_chat_and_syncs_fts() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(
                USER,
                "what is the difference betw",
                None,
                None,
                "hero-llama",
                vec![],
            )
            .unwrap();

        let applied = store
            .auto_title_chat(USER, chat.id, "A Good Title")
            .unwrap();
        assert!(applied);

        let updated = store.get_chat(USER, chat.id).unwrap().chat;
        assert_eq!(updated.title, "A Good Title");

        let hits = store.search_chats(USER, "Good").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, chat.id);
    }

    /// A user rename always wins: `rename_chat` clears `title_auto` to 0,
    /// so a later `auto_title_chat` call is a no-op — `Ok(false)`, the
    /// user's title unchanged.
    #[test]
    fn t18_auto_title_chat_never_overwrites_a_user_rename() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(
                USER,
                "first line of the message",
                None,
                None,
                "hero-llama",
                vec![],
            )
            .unwrap();
        store.rename_chat(USER, chat.id, "User Name").unwrap();
        assert_eq!(title_auto_of(&store, USER, chat.id), 0);

        let applied = store
            .auto_title_chat(USER, chat.id, "Model Title")
            .unwrap();
        assert!(!applied);

        let after = store.get_chat(USER, chat.id).unwrap().chat;
        assert_eq!(after.title, "User Name");
    }

    /// An empty/whitespace-only title is a no-op — `Ok(false)`, title
    /// unchanged (the FE should never send one; this is defense-in-depth).
    #[test]
    fn t19_auto_title_chat_rejects_empty_or_whitespace_title() {
        let store = ConvStore::new_in_memory();
        let chat = store
            .create_chat(USER, "Original", None, None, "hero-llama", vec![])
            .unwrap();

        assert!(!store.auto_title_chat(USER, chat.id, "").unwrap());
        assert!(!store.auto_title_chat(USER, chat.id, "   \n\t  ").unwrap());

        let after = store.get_chat(USER, chat.id).unwrap().chat;
        assert_eq!(after.title, "Original");
    }

    /// Account-scoping (mirrors `t9`/`t15`): B cannot auto-title A's chat —
    /// a foreign id simply matches no row in B's own database. Per
    /// `t9`'s doc comment, `a_chat.id` is very likely ALSO B's own first
    /// chat id (each account's ids start their own sequence at 1), so
    /// `auto_title_chat("acct-b", a_chat.id, ..)` legitimately returning
    /// `Ok(true)` is expected — that's B retitling B's OWN row at that
    /// number, never A's. The actual security property, proven below
    /// regardless of the return value: A's chat title must never change.
    #[test]
    fn t20_auto_title_chat_is_account_scoped_b_cannot_retitle_as_chat() {
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
            .create_chat(
                "acct-b",
                "B's private chat",
                None,
                None,
                "hero-llama",
                vec![],
            )
            .unwrap();

        let _ = store
            .auto_title_chat("acct-b", a_chat.id, "Hijacked title")
            .unwrap();

        let a_after = store.get_chat("acct-a", a_chat.id).unwrap().chat;
        assert_eq!(
            a_after.title, "A's private chat",
            "A's title must be unchanged no matter what B's call returned"
        );
    }

    /// Defensive migration (mirrors `t8`): a `chats` table created with the
    /// pre-S7-5 columns (i.e. `model_id`/`adapter_ids` already present, but
    /// no `title_auto`) gains `title_auto` (default 1) via
    /// `migrate_chats_columns`, and `auto_title_chat` then works on its
    /// rows. Exercises the real open path (`ConvStore::new` + a real
    /// directory), not just `migrate_chats_columns` in isolation.
    #[test]
    fn t21_migrates_an_existing_per_account_db_missing_title_auto() {
        let dir = std::env::temp_dir().join(format!(
            "cleophis-convstore-migrate-title-auto-test-{}-{}",
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
                    mounted_packs TEXT,
                    model_id TEXT NOT NULL DEFAULT '',
                    adapter_ids TEXT NOT NULL DEFAULT '[]'
                );",
            )
            .unwrap();
        }

        let store = ConvStore::new(dir.clone());
        let chat = store
            .create_chat(USER, "Migrated", None, None, "hero-llama", vec![])
            .unwrap();
        assert_eq!(title_auto_of(&store, USER, chat.id), 1);

        let applied = store
            .auto_title_chat(USER, chat.id, "Post-migration title")
            .unwrap();
        assert!(applied);
        let after = store.get_chat(USER, chat.id).unwrap().chat;
        assert_eq!(after.title, "Post-migration title");

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn oa6_convstore_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cleophis-convstore-oa6-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Task 6 ("remove account from this device")'s chats-side wipe:
    /// deletes the account's per-account `.db` file.
    #[test]
    fn t22_delete_account_conversations_deletes_the_per_account_db_file() {
        let dir = oa6_convstore_dir("delete-db");
        let store = ConvStore::new(dir.clone());
        store
            .create_chat(USER, "to be wiped", None, None, "hero-llama", vec![])
            .unwrap();
        let db_path = dir.join(format!("{USER}.db"));
        assert!(db_path.exists(), "setup: the per-account db file should exist");

        store.delete_account_conversations(USER).expect("should delete");
        assert!(!db_path.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An account that never created a chat (no `.db` file at all) is a
    /// clean no-op, not an error — idempotent.
    #[test]
    fn t23_delete_account_conversations_missing_db_is_not_an_error() {
        let dir = oa6_convstore_dir("delete-db-missing");
        let store = ConvStore::new(dir.clone());

        store
            .delete_account_conversations("user-oa6-never-chatted")
            .expect("missing db should be a clean no-op");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// THE traversal-rejection assertion (SECURITY): a malformed id is a
    /// hard `Err`, never a best-effort/traversal-prone path join.
    #[test]
    fn t24_delete_account_conversations_rejects_traversal_user_id() {
        let dir = oa6_convstore_dir("delete-db-traversal");
        let store = ConvStore::new(dir.clone());

        let err = store.delete_account_conversations("../evil").unwrap_err();
        assert!(!err.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Removing one account's conversations never touches a sibling
    /// account's `.db` file or its rows — same cross-account boundary
    /// `t9_per_account_isolation_...` proves for reads, here for the
    /// whole-database wipe.
    #[test]
    fn t25_delete_account_conversations_leaves_a_different_accounts_db_untouched() {
        let dir = oa6_convstore_dir("delete-db-cross-account");
        let store = ConvStore::new(dir.clone());
        store
            .create_chat("acct-oa6-a", "A's chat", None, None, "hero-llama", vec![])
            .unwrap();
        store
            .create_chat("acct-oa6-b", "B's chat", None, None, "hero-llama", vec![])
            .unwrap();

        store
            .delete_account_conversations("acct-oa6-a")
            .expect("should delete A's db");

        assert!(!dir.join("acct-oa6-a.db").exists());
        assert!(
            dir.join("acct-oa6-b.db").exists(),
            "B's db file must survive A's removal"
        );
        assert_eq!(
            store.list_chats("acct-oa6-b").unwrap().len(),
            1,
            "B's chats must survive A's removal"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
