use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use bm25::SearchEngine;
use rusqlite::{params, Connection};
use std::sync::Mutex;

use anyhow::Context;
use tracing::warn;

use crate::dedup_window::{DedupVerdict, WindowedDedup};

pub type DbErr = rusqlite::Error;

pub const DEFAULT_COLLECTION_NAME: &str = "mempalace_drawers";
pub const DEFAULT_COMPRESSED_COLLECTION_NAME: &str = "mempalace_compressed";

/// Process-global online dedup window (mp-032 / report 06 §3.5).
///
/// Initialised lazily on first use with [`WindowedDedup::default`]
/// (5-minute window, 4096-entry LRU). The same instance is shared across
/// every `PalaceDb` opened in this process so multiple short-lived
/// `PalaceDb::open` calls (e.g. the MCP `tool_add_drawer` flow that
/// re-opens per request) still benefit from the rolling window.
///
/// Returns an `Arc` so callers can either
/// (a) use the returned handle directly, or
/// (b) hand it to [`PalaceDb::add_drawer_with_dedup`] for tests that
/// want isolated state.
pub fn dedup_window() -> Arc<WindowedDedup> {
    static GLOBAL: OnceLock<Arc<WindowedDedup>> = OnceLock::new();
    GLOBAL
        .get_or_init(|| Arc::new(WindowedDedup::default()))
        .clone()
}

/// Distinct lifecycle states of a palace on disk (#1498).
///
/// The upstream Python port (`fix(palace): stratify state messages for
/// empty/missing palace`, milla-jovovich/mempalace#1498) split the single
/// "No palace found / run init" message into three actionable states so the
/// CLI no longer tells a user to re-run `init` after `init` has already
/// succeeded but `mine` has not. The Rust port carries the same distinction
/// here, applied by `cmd_status`, `cmd_compress`, and the search error path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PalaceState {
    /// Palace directory does not exist (or is not a directory).
    Missing,
    /// Palace directory exists but the collection JSON has not been written
    /// yet — `init` ran, `mine` did not.
    NotInitialized,
    /// Collection JSON exists but contains no documents.
    Empty,
    /// Palace is initialized and contains at least one drawer.
    Ready,
}

/// Classify the current state of `palace_path` (#1498).
///
/// Cheap, side-effect-free: only touches the filesystem (existence checks +
/// one JSON read when present). Safe to call from any CLI command before
/// opening a `PalaceDb` to choose an actionable user-facing message.
pub fn classify_palace(palace_path: &std::path::Path) -> PalaceState {
    classify_palace_for_collection(palace_path, DEFAULT_COLLECTION_NAME)
}

/// Collection-aware variant of [`classify_palace`] (#1498).
pub fn classify_palace_for_collection(
    palace_path: &std::path::Path,
    collection_name: &str,
) -> PalaceState {
    if !palace_path.is_dir() {
        return PalaceState::Missing;
    }
    let docs_path = palace_path.join(format!("{}.json", collection_name));
    if !docs_path.is_file() {
        return PalaceState::NotInitialized;
    }
    // Mirror PalaceDb::open: missing/unparseable JSON degrades to empty.
    let content = match std::fs::read_to_string(&docs_path) {
        Ok(c) => c,
        Err(_) => return PalaceState::NotInitialized,
    };
    let docs: HashMap<String, DocumentEntry> = match serde_json::from_str(&content) {
        Ok(docs) => docs,
        Err(e) => {
            warn!("corrupted collection file at {}: {}", docs_path.display(), e);
            HashMap::new()
        }
    };
    if docs.is_empty() {
        PalaceState::Empty
    } else {
        PalaceState::Ready
    }
}

/// Print the actionable next-step hint for a non-`Ready` palace state (#1498).
///
/// Returns `true` when a message was printed (state was not `Ready`) so the
/// caller can decide whether to short-circuit. The leading newline matches the
/// pre-existing print style used elsewhere in the CLI.
pub fn print_palace_state_hint(state: PalaceState, palace_path: &std::path::Path) -> bool {
    match state {
        PalaceState::Missing => {
            println!("\n  No palace found at {}", palace_path.display());
            println!("  Run: mpr init <dir>");
            true
        }
        PalaceState::NotInitialized => {
            println!(
                "\n  Palace directory exists at {} but no data has been mined yet.",
                palace_path.display()
            );
            println!("  Run: mpr mine <dir>");
            true
        }
        PalaceState::Empty => {
            println!(
                "\n  Palace at {} has no drawers yet.",
                palace_path.display()
            );
            println!("  Run: mpr mine <dir> to ingest content.");
            true
        }
        PalaceState::Ready => false,
    }
}

/// Atomically write data to a file with fsync protection.
/// Uses the write-to-temp-then-rename pattern.
pub fn atomic_write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp_path = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp_path)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp_path, path)?;
    // Sync the parent directory
    if let Some(parent) = path.parent() {
        if let Ok(f) = std::fs::File::open(parent) {
            f.sync_all().ok();
        }
    }
    Ok(())
}

pub struct PalaceDb {
    documents: HashMap<String, DocumentEntry>,
    palace_path: PathBuf,
    collection_name: String,
    coordination: Arc<Mutex<CoordinationDb>>,
    bm25: SearchEngine<String>,
    embedder: Arc<dyn crate::embed::Embedder>,
    /// Optional HNSW vector index. `None` when opened without a real embedder
    /// (the "naive" Jaccard path). Populated lazily when first vector search
    /// is requested, or eagerly by `open_with_embedder`.
    embedding_db: Option<EmbeddingDb>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct DocumentEntry {
    pub content: String,
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct QueryResult {
    pub ids: Vec<String>,
    pub documents: Vec<String>,
    pub distances: Vec<f64>,
    pub metadatas: Vec<HashMap<String, serde_json::Value>>,
}

pub struct EmbeddingDb {
    embedder: Arc<dyn crate::embed::Embedder>,
    hnsw: embedvec::HnswIndex,
    #[allow(dead_code)]
    documents: Vec<(String, String)>,
    storage: embedvec::VectorStorage,
}

// ---------------------------------------------------------------------------
// Coordination database (actions, leases, routines, signals)
// ---------------------------------------------------------------------------

pub struct CoordinationDb {
    conn: Connection,
}

impl std::ops::Deref for CoordinationDb {
    type Target = Connection;
    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Action {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub priority: i64,
    pub project: String,
    pub tags: String,
    pub parent_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Lease {
    pub id: String,
    pub action_id: String,
    pub agent_id: String,
    pub status: String,
    pub result: Option<String>,
    pub ttl_ms: i64,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Routine {
    pub id: String,
    pub name: String,
    pub steps: String,
    pub created_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Signal {
    pub id: String,
    pub from_agent: String,
    pub to_agent: String,
    pub content: String,
    pub signal_type: String,
    pub reply_to: Option<String>,
    pub read: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemorySlot {
    pub id: String,
    pub label: String,
    pub content: String,
    pub size_limit: i32,
    pub description: String,
    pub pinned: bool,
    pub scope: String,
    pub project: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SketchRecord {
    pub id: String,
    pub title: String,
    pub description: String,
    pub steps: String,
    pub project: String,
    pub expires_at: String,
    pub created_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CrystalRecord {
    pub id: String,
    pub action_ids: String,
    pub summary: String,
    pub narrative: String,
    pub outcomes: String,
    pub files_affected: String,
    pub lessons: String,
    pub project: String,
    pub session_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FacetRecord {
    pub id: String,
    pub target_id: String,
    pub target_type: String,
    pub dimension: String,
    pub value: String,
    pub created_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LessonRecord {
    pub id: String,
    pub content: String,
    pub context: String,
    pub confidence: f64,
    pub project: String,
    pub tags: String,
    pub reinforced_at: String,
    pub created_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InsightRecord {
    pub id: String,
    pub content: String,
    pub confidence: f64,
    pub project: String,
    pub cluster_id: String,
    pub reinforced_count: i32,
    pub created_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Sentinel {
    pub id: String,
    pub name: String,
    pub watch_type: String,
    pub trigger_condition: String,
    pub action_id: Option<String>,
    pub expires_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Checkpoint {
    pub id: String,
    pub name: String,
    pub operation: String,
    pub status: Option<String>,
    pub checkpoint_type: String,
    pub linked_action_ids: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TeamShare {
    pub id: String,
    pub item_id: String,
    pub item_type: String,
    pub project: String,
    pub shared_at: String,
}

impl CoordinationDb {
    pub fn open(path: &std::path::Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(path)?;
        let db_path = path.join("coordination.db");
        let conn = Connection::open(&db_path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS actions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'pending',
                priority INTEGER NOT NULL DEFAULT 5,
                project TEXT NOT NULL DEFAULT '',
                tags TEXT NOT NULL DEFAULT '',
                parent_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS action_dependencies (
                action_id TEXT NOT NULL,
                depends_on_action_id TEXT NOT NULL,
                PRIMARY KEY (action_id, depends_on_action_id),
                FOREIGN KEY (action_id) REFERENCES actions(id) ON DELETE CASCADE,
                FOREIGN KEY (depends_on_action_id) REFERENCES actions(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS leases (
                id TEXT PRIMARY KEY,
                action_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'active',
                result TEXT,
                ttl_ms INTEGER NOT NULL DEFAULT 300000,
                created_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                FOREIGN KEY (action_id) REFERENCES actions(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS routines (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                steps TEXT NOT NULL DEFAULT '[]',
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS signals (
                id TEXT PRIMARY KEY,
                from_agent TEXT NOT NULL,
                to_agent TEXT NOT NULL,
                content TEXT NOT NULL,
                signal_type TEXT NOT NULL DEFAULT 'info',
                reply_to TEXT,
                read INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS sentinels (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                watch_type TEXT NOT NULL,
                trigger_condition TEXT NOT NULL,
                action_id TEXT,
                expires_at TEXT,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS checkpoints (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                operation TEXT NOT NULL,
                status TEXT,
                checkpoint_type TEXT NOT NULL DEFAULT 'manual',
                linked_action_ids TEXT NOT NULL DEFAULT '[]',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS team_shares (
                id TEXT PRIMARY KEY,
                item_id TEXT NOT NULL,
                item_type TEXT NOT NULL,
                project TEXT NOT NULL DEFAULT '',
                shared_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_actions_status ON actions(status);
            CREATE INDEX IF NOT EXISTS idx_actions_project ON actions(project);
            CREATE INDEX IF NOT EXISTS idx_leases_action_id ON leases(action_id);
            CREATE INDEX IF NOT EXISTS idx_leases_status ON leases(status);
            CREATE INDEX IF NOT EXISTS idx_signals_to_agent ON signals(to_agent);
            CREATE INDEX IF NOT EXISTS idx_signals_read ON signals(read);
            CREATE TABLE IF NOT EXISTS slots (
                id TEXT PRIMARY KEY,
                label TEXT UNIQUE NOT NULL,
                content TEXT NOT NULL DEFAULT '',
                size_limit INTEGER NOT NULL DEFAULT 2000,
                description TEXT NOT NULL DEFAULT '',
                pinned INTEGER NOT NULL DEFAULT 1,
                scope TEXT NOT NULL DEFAULT 'project',
                project TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_slots_label ON slots(label);
            CREATE INDEX IF NOT EXISTS idx_slots_project ON slots(project);
            CREATE TABLE IF NOT EXISTS sketches (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                steps TEXT NOT NULL DEFAULT '[]',
                project TEXT NOT NULL DEFAULT '',
                expires_at TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS facets (
                id TEXT PRIMARY KEY,
                target_id TEXT NOT NULL,
                target_type TEXT NOT NULL,
                dimension TEXT NOT NULL,
                value TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_facets_target ON facets(target_id, target_type);
            CREATE INDEX IF NOT EXISTS idx_facets_dimension ON facets(dimension);
            CREATE TABLE IF NOT EXISTS crystals (
                id TEXT PRIMARY KEY,
                action_ids TEXT NOT NULL DEFAULT '',
                summary TEXT NOT NULL DEFAULT '',
                narrative TEXT NOT NULL DEFAULT '',
                outcomes TEXT NOT NULL DEFAULT '',
                files_affected TEXT NOT NULL DEFAULT '',
                lessons TEXT NOT NULL DEFAULT '',
                project TEXT NOT NULL DEFAULT '',
                session_id TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_crystals_project ON crystals(project);
            CREATE TABLE IF NOT EXISTS lessons (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL DEFAULT '',
                context TEXT NOT NULL DEFAULT '',
                confidence REAL NOT NULL DEFAULT 0.5,
                project TEXT NOT NULL DEFAULT '',
                tags TEXT NOT NULL DEFAULT '',
                reinforced_at TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_lessons_confidence ON lessons(confidence);
            CREATE INDEX IF NOT EXISTS idx_lessons_project ON lessons(project);
            CREATE TABLE IF NOT EXISTS insights (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL DEFAULT '',
                confidence REAL NOT NULL DEFAULT 0.0,
                project TEXT NOT NULL DEFAULT '',
                cluster_id TEXT NOT NULL DEFAULT '',
                reinforced_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_insights_confidence ON insights(confidence);
            CREATE INDEX IF NOT EXISTS idx_insights_project ON insights(project);",
        )?;
        Ok(Self { conn })
    }

    // Actions
    pub fn action_create(&mut self, action: &Action) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO actions (id, title, description, status, priority, project, tags, parent_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                action.id,
                action.title,
                action.description,
                action.status,
                action.priority,
                action.project,
                action.tags,
                action.parent_id,
                action.created_at,
                action.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn action_update(
        &mut self,
        id: &str,
        status: Option<&str>,
        result: Option<&str>,
        priority: Option<i64>,
    ) -> anyhow::Result<bool> {
        let mut updated = false;
        if let Some(s) = status {
            self.conn.execute(
                "UPDATE actions SET status = ?1, updated_at = ?2 WHERE id = ?3",
                params![s, chrono::Utc::now().to_rfc3339(), id],
            )?;
            updated = true;
        }
        if let Some(r) = result {
            self.conn.execute(
                "UPDATE actions SET description = ?1, updated_at = ?2 WHERE id = ?3",
                params![r, chrono::Utc::now().to_rfc3339(), id],
            )?;
            updated = true;
        }
        if let Some(p) = priority {
            self.conn.execute(
                "UPDATE actions SET priority = ?1, updated_at = ?2 WHERE id = ?3",
                params![p, chrono::Utc::now().to_rfc3339(), id],
            )?;
            updated = true;
        }
        Ok(updated)
    }

    pub fn action_get(&self, id: &str) -> anyhow::Result<Option<Action>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, title, description, status, priority, project, tags, parent_id, created_at, updated_at FROM actions WHERE id = ?1")?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Action {
                id: row.get(0)?,
                title: row.get(1)?,
                description: row.get(2)?,
                status: row.get(3)?,
                priority: row.get(4)?,
                project: row.get(5)?,
                tags: row.get(6)?,
                parent_id: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn action_add_dependency(
        &mut self,
        action_id: &str,
        depends_on: &str,
    ) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO action_dependencies (action_id, depends_on_action_id) VALUES (?1, ?2)",
            params![action_id, depends_on],
        )?;
        Ok(())
    }

    pub fn action_list_unblocked(
        &self,
        project: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<Action>> {
        let query = if project.is_some() {
            "SELECT a.id, a.title, a.description, a.status, a.priority, a.project, a.tags, a.parent_id, a.created_at, a.updated_at
             FROM actions a
             WHERE a.status = 'pending'
             AND NOT EXISTS (
                 SELECT 1 FROM action_dependencies ad
                 JOIN actions dep ON dep.id = ad.depends_on_action_id
                 WHERE ad.action_id = a.id AND dep.status NOT IN ('done', 'cancelled')
             )
             AND (?1 = '' OR a.project = ?1)
             ORDER BY a.priority DESC, a.created_at ASC
             LIMIT ?2"
        } else {
            "SELECT a.id, a.title, a.description, a.status, a.priority, a.project, a.tags, a.parent_id, a.created_at, a.updated_at
             FROM actions a
             WHERE a.status = 'pending'
             AND NOT EXISTS (
                 SELECT 1 FROM action_dependencies ad
                 JOIN actions dep ON dep.id = ad.depends_on_action_id
                 WHERE ad.action_id = a.id AND dep.status NOT IN ('done', 'cancelled')
             )
             ORDER BY a.priority DESC, a.created_at ASC
             LIMIT ?1"
        };
        let mut stmt = self.conn.prepare(query)?;
        let project_val = project.unwrap_or("");
        let mut rows = if project.is_some() {
            stmt.query(params![project_val, limit as i64])?
        } else {
            stmt.query(params![limit as i64])?
        };
        let mut actions = Vec::new();
        while let Some(row) = rows.next()? {
            actions.push(Action {
                id: row.get(0)?,
                title: row.get(1)?,
                description: row.get(2)?,
                status: row.get(3)?,
                priority: row.get(4)?,
                project: row.get(5)?,
                tags: row.get(6)?,
                parent_id: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            });
        }
        Ok(actions)
    }

    // Leases
    pub fn lease_create(&mut self, lease: &Lease) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO leases (id, action_id, agent_id, status, result, ttl_ms, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                lease.id,
                lease.action_id,
                lease.agent_id,
                lease.status,
                lease.result,
                lease.ttl_ms,
                lease.created_at,
                lease.expires_at,
            ],
        )?;
        Ok(())
    }

    pub fn lease_get_active(&self, action_id: &str) -> anyhow::Result<Option<Lease>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, action_id, agent_id, status, result, ttl_ms, created_at, expires_at
             FROM leases WHERE action_id = ?1 AND status = 'active' AND expires_at > ?2",
        )?;
        let now = chrono::Utc::now().to_rfc3339();
        let mut rows = stmt.query(params![action_id, now])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Lease {
                id: row.get(0)?,
                action_id: row.get(1)?,
                agent_id: row.get(2)?,
                status: row.get(3)?,
                result: row.get(4)?,
                ttl_ms: row.get(5)?,
                created_at: row.get(6)?,
                expires_at: row.get(7)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn lease_release(&mut self, id: &str) -> anyhow::Result<bool> {
        let rows = self.conn.execute(
            "UPDATE leases SET status = 'released' WHERE id = ?1",
            params![id],
        )?;
        Ok(rows > 0)
    }

    pub fn lease_renew(&mut self, id: &str, ttl_ms: i64) -> anyhow::Result<bool> {
        let new_expires =
            (chrono::Utc::now() + chrono::Duration::milliseconds(ttl_ms)).to_rfc3339();
        let rows = self.conn.execute(
            "UPDATE leases SET expires_at = ?1 WHERE id = ?2 AND status = 'active'",
            params![new_expires, id],
        )?;
        Ok(rows > 0)
    }

    // Routines
    pub fn routine_create(&mut self, routine: &Routine) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO routines (id, name, steps, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![routine.id, routine.name, routine.steps, routine.created_at],
        )?;
        Ok(())
    }

    pub fn action_list_all(&self) -> anyhow::Result<Vec<Action>> {
        let mut stmt = self.conn.prepare(
            "SELECT a.id, a.title, a.description, a.status, a.priority, a.project, a.tags, a.parent_id, a.created_at, a.updated_at
             FROM actions a
             ORDER BY a.created_at DESC",
        )?;
        let mut rows = stmt.query([])?;
        let mut actions = Vec::new();
        while let Some(row) = rows.next()? {
            actions.push(Action {
                id: row.get(0)?,
                title: row.get(1)?,
                description: row.get(2)?,
                status: row.get(3)?,
                priority: row.get(4)?,
                project: row.get(5)?,
                tags: row.get(6)?,
                parent_id: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            });
        }
        Ok(actions)
    }

    pub fn lease_list_all(&self) -> anyhow::Result<Vec<Lease>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, action_id, agent_id, status, result, ttl_ms, created_at, expires_at
             FROM leases
             ORDER BY created_at DESC",
        )?;
        let mut rows = stmt.query([])?;
        let mut leases = Vec::new();
        while let Some(row) = rows.next()? {
            leases.push(Lease {
                id: row.get(0)?,
                action_id: row.get(1)?,
                agent_id: row.get(2)?,
                status: row.get(3)?,
                result: row.get(4)?,
                ttl_ms: row.get(5)?,
                created_at: row.get(6)?,
                expires_at: row.get(7)?,
            });
        }
        Ok(leases)
    }

    pub fn routine_list_all(&self) -> anyhow::Result<Vec<Routine>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, steps, created_at FROM routines ORDER BY created_at DESC")?;
        let mut rows = stmt.query([])?;
        let mut routines = Vec::new();
        while let Some(row) = rows.next()? {
            routines.push(Routine {
                id: row.get(0)?,
                name: row.get(1)?,
                steps: row.get(2)?,
                created_at: row.get(3)?,
            });
        }
        Ok(routines)
    }

    pub fn signal_list_all(&self) -> anyhow::Result<Vec<Signal>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, from_agent, to_agent, content, signal_type, reply_to, read, created_at
             FROM signals ORDER BY created_at DESC",
        )?;
        let mut rows = stmt.query([])?;
        let mut signals = Vec::new();
        while let Some(row) = rows.next()? {
            signals.push(Signal {
                id: row.get(0)?,
                from_agent: row.get(1)?,
                to_agent: row.get(2)?,
                content: row.get(3)?,
                signal_type: row.get(4)?,
                reply_to: row.get(5)?,
                read: row.get(6)?,
                created_at: row.get(7)?,
            });
        }
        Ok(signals)
    }

    pub fn routine_get(&self, id: &str) -> anyhow::Result<Option<Routine>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, steps, created_at FROM routines WHERE id = ?1")?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Routine {
                id: row.get(0)?,
                name: row.get(1)?,
                steps: row.get(2)?,
                created_at: row.get(3)?,
            }))
        } else {
            Ok(None)
        }
    }

    // Signals
    pub fn signal_create(&mut self, signal: &Signal) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO signals (id, from_agent, to_agent, content, signal_type, reply_to, read, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                signal.id,
                signal.from_agent,
                signal.to_agent,
                signal.content,
                signal.signal_type,
                signal.reply_to,
                signal.read as i32,
                signal.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn signal_list(
        &self,
        agent_id: &str,
        unread_only: bool,
        _thread_id: Option<&str>,
        limit: usize,
    ) -> anyhow::Result<Vec<Signal>> {
        let query = if unread_only {
            "SELECT id, from_agent, to_agent, content, signal_type, reply_to, read, created_at
             FROM signals WHERE to_agent = ?1 AND read = 0
             LIMIT ?2"
        } else {
            "SELECT id, from_agent, to_agent, content, signal_type, reply_to, read, created_at
             FROM signals WHERE to_agent = ?1
             LIMIT ?2"
        };
        let mut stmt = self.conn.prepare(query)?;
        let limit_i64 = limit as i64;
        let mut rows = stmt.query(params![agent_id, limit_i64])?;
        let mut signals = Vec::new();
        while let Some(row) = rows.next()? {
            signals.push(Signal {
                id: row.get(0)?,
                from_agent: row.get(1)?,
                to_agent: row.get(2)?,
                content: row.get(3)?,
                signal_type: row.get(4)?,
                reply_to: row.get(5)?,
                read: row.get::<_, i32>(6)? != 0,
                created_at: row.get(7)?,
            });
        }
        Ok(signals)
    }

    pub fn signal_mark_read(&mut self, id: &str) -> anyhow::Result<bool> {
        let rows = self
            .conn
            .execute("UPDATE signals SET read = 1 WHERE id = ?1", params![id])?;
        Ok(rows > 0)
    }

    pub fn sentinel_create(&self, sentinel: &Sentinel) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO sentinels (id, name, watch_type, trigger_condition, action_id, expires_at, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                sentinel.id,
                sentinel.name,
                sentinel.watch_type,
                sentinel.trigger_condition,
                sentinel.action_id,
                sentinel.expires_at,
                sentinel.created_at
            ],
        )?;
        Ok(())
    }

    pub fn sentinel_list(&self) -> anyhow::Result<Vec<Sentinel>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, watch_type, trigger_condition, action_id, expires_at, created_at FROM sentinels ORDER BY created_at DESC"
        )?;
        let mut rows = stmt.query([])?;
        let mut sentinels = Vec::new();
        while let Some(row) = rows.next()? {
            sentinels.push(Sentinel {
                id: row.get(0)?,
                name: row.get(1)?,
                watch_type: row.get(2)?,
                trigger_condition: row.get(3)?,
                action_id: row.get(4)?,
                expires_at: row.get(5)?,
                created_at: row.get(6)?,
            });
        }
        Ok(sentinels)
    }

    pub fn sentinel_get(&self, sentinel_id: &str) -> anyhow::Result<Option<Sentinel>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, watch_type, trigger_condition, action_id, expires_at, created_at FROM sentinels WHERE id = ?1"
        )?;
        let mut rows = stmt.query(params![sentinel_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Sentinel {
                id: row.get(0)?,
                name: row.get(1)?,
                watch_type: row.get(2)?,
                trigger_condition: row.get(3)?,
                action_id: row.get(4)?,
                expires_at: row.get(5)?,
                created_at: row.get(6)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn sentinel_delete(&self, sentinel_id: &str) -> anyhow::Result<()> {
        self.conn
            .execute("DELETE FROM sentinels WHERE id = ?1", params![sentinel_id])?;
        Ok(())
    }

    pub fn checkpoint_create(&self, checkpoint: &Checkpoint) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO checkpoints (id, name, operation, status, checkpoint_type, linked_action_ids, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                checkpoint.id,
                checkpoint.name,
                checkpoint.operation,
                checkpoint.status,
                checkpoint.checkpoint_type,
                checkpoint.linked_action_ids,
                checkpoint.created_at,
                checkpoint.updated_at
            ],
        )?;
        Ok(())
    }

    pub fn checkpoint_resolve(&self, checkpoint_id: &str, status: &str) -> anyhow::Result<()> {
        self.conn.execute(
            "UPDATE checkpoints SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![status, checkpoint_id],
        )?;
        Ok(())
    }

    pub fn checkpoint_list(&self) -> anyhow::Result<Vec<Checkpoint>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, operation, status, checkpoint_type, linked_action_ids, created_at, updated_at FROM checkpoints ORDER BY created_at DESC"
        )?;
        let mut rows = stmt.query([])?;
        let mut checkpoints = Vec::new();
        while let Some(row) = rows.next()? {
            checkpoints.push(Checkpoint {
                id: row.get(0)?,
                name: row.get(1)?,
                operation: row.get(2)?,
                status: row.get(3)?,
                checkpoint_type: row.get(4)?,
                linked_action_ids: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            });
        }
        Ok(checkpoints)
    }

    pub fn team_share_create(&self, share: &TeamShare) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO team_shares (id, item_id, item_type, project, shared_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                share.id,
                share.item_id,
                share.item_type,
                share.project,
                share.shared_at
            ],
        )?;
        Ok(())
    }

    pub fn team_share_list(&self, project: Option<&str>) -> anyhow::Result<Vec<TeamShare>> {
        let query = match project {
            Some(_) => "SELECT id, item_id, item_type, project, shared_at FROM team_shares WHERE project = ?1 ORDER BY shared_at DESC",
            None => "SELECT id, item_id, item_type, project, shared_at FROM team_shares ORDER BY shared_at DESC",
        };
        let mut stmt = self.conn.prepare(query)?;
        let shares = if let Some(p) = project {
            let mut rows = stmt.query(params![p])?;
            let mut result = Vec::new();
            while let Some(row) = rows.next()? {
                result.push(TeamShare {
                    id: row.get(0)?,
                    item_id: row.get(1)?,
                    item_type: row.get(2)?,
                    project: row.get(3)?,
                    shared_at: row.get(4)?,
                });
            }
            result
        } else {
            let mut rows = stmt.query([])?;
            let mut result = Vec::new();
            while let Some(row) = rows.next()? {
                result.push(TeamShare {
                    id: row.get(0)?,
                    item_id: row.get(1)?,
                    item_type: row.get(2)?,
                    project: row.get(3)?,
                    shared_at: row.get(4)?,
                });
            }
            result
        };
        Ok(shares)
    }
}

impl EmbeddingDb {
    /// Return the text of the drawer at index `idx`, or `None` if out of range.
    pub(crate) fn nth_text(&self, idx: usize) -> Option<&str> {
        self.documents.get(idx).map(|(_, t)| t.as_str())
    }

    /// Return the id of the drawer at index `idx`, or `None` if out of range.
    pub(crate) fn id_at(&self, idx: usize) -> Option<String> {
        self.documents.get(idx).map(|(id, _)| id.clone())
    }

    /// Return the number of drawers stored.
    pub(crate) fn len(&self) -> usize {
        self.documents.len()
    }

    /// Save the embedding index to disk at `cache_path`.
    /// Format: one binary file with header (magic, dim, count) + raw f32 vectors,
    /// plus a JSON sidecar for document metadata.
    pub fn save_cache(&self, cache_path: &std::path::Path) -> anyhow::Result<()> {
        use std::io::Write;
        let dim = self.embedder.dim();
        let count = self.documents.len();
        if count == 0 {
            return Ok(());
        }

        // Collect all vectors from storage
        let mut raw = Vec::with_capacity(count * dim);
        for i in 0..count {
            match self.storage.get(i, None) {
                Ok(v) => raw.extend(v),
                Err(_) => anyhow::bail!("failed to read vector at index {}", i),
            }
        }

        // Write binary: magic(8) + dim(8) + count(8) + raw_data
        let mut bin = std::fs::File::create(cache_path.with_extension("bin"))?;
        bin.write_all(b"EMBEDVEC")?; // magic
        bin.write_all(&(dim as u64).to_le_bytes())?;
        bin.write_all(&(count as u64).to_le_bytes())?;
        // Write raw f32 vectors as 4-byte little-endian
        for &val in &raw {
            bin.write_all(&val.to_le_bytes())?;
        }
        bin.flush()?;

        // Write documents JSON
        let docs: Vec<Vec<String>> = self
            .documents
            .iter()
            .map(|(id, text)| vec![id.clone(), text.clone()])
            .collect();
        let json = serde_json::to_string(&docs)?;
        std::fs::write(cache_path.with_extension("json"), &json)?;

        Ok(())
    }

    /// Load a previously saved embedding cache from `cache_path`.
    /// Builds HNSW index + storage from the cached vectors (no re-embedding).
    pub fn load_cache(
        cache_path: &std::path::Path,
        embedder: Arc<dyn crate::embed::Embedder>,
    ) -> anyhow::Result<Self> {
        use std::io::Read;
        let dim = embedder.dim();

        // Read binary
        let mut bin = std::fs::File::open(cache_path.with_extension("bin"))?;
        let mut magic = [0u8; 8];
        bin.read_exact(&mut magic)?;
        anyhow::ensure!(&magic == b"EMBEDVEC", "bad embedding cache magic");

        let mut buf = [0u8; 8];
        bin.read_exact(&mut buf)?;
        let stored_dim = u64::from_le_bytes(buf) as usize;
        bin.read_exact(&mut buf)?;
        let count = u64::from_le_bytes(buf) as usize;
        anyhow::ensure!(
            stored_dim == dim,
            "dim mismatch: cached={stored_dim} embedder={dim}"
        );

        let mut vectors = Vec::with_capacity(count * dim);
        let mut buf = [0u8; 4];
        for _ in 0..count * dim {
            bin.read_exact(&mut buf)?;
            vectors.push(f32::from_le_bytes(buf));
        }

        // Build storage + HNSW from cached vectors
        let mut storage = embedvec::VectorStorage::new(dim, embedvec::Quantization::None);
        let mut hnsw = embedvec::HnswIndex::new(16, 200, embedvec::Distance::Cosine);
        for i in 0..count {
            let start = i * dim;
            let vec = &vectors[start..start + dim];
            storage.add(vec, None)?;
            hnsw.insert(i, vec, &storage, None)?;
        }

        // Load documents
        let json = std::fs::read_to_string(cache_path.with_extension("json"))?;
        let docs: Vec<Vec<String>> = serde_json::from_str(&json)?;
        let documents: Vec<(String, String)> = docs
            .into_iter()
            .map(|d| (d[0].clone(), d[1].clone()))
            .collect();

        Ok(Self {
            embedder,
            hnsw,
            documents,
            storage,
        })
    }
}

/// Run an async embedding future to completion from a synchronous context,
/// whether or not a tokio runtime is already active on the current thread.
///
/// When called from inside a runtime (the normal MCP/CLI/test case), a bare
/// `Handle::block_on` panics with "Cannot start a runtime from within a
/// runtime". We instead offload to a dedicated OS thread that owns its own
/// runtime — fastembed's internal `spawn_blocking` still works because that
/// thread has a live runtime. Outside any runtime we just build one inline.
fn run_off_runtime<T, Fut, M>(make: M) -> anyhow::Result<T>
where
    M: FnOnce() -> Fut + Send,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
    T: Send,
{
    let run = move || -> anyhow::Result<T> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        rt.block_on(make())
    };
    if tokio::runtime::Handle::try_current().is_ok() {
        std::thread::scope(|s| {
            s.spawn(run)
                .join()
                .map_err(|_| anyhow::anyhow!("embedding worker thread panicked"))?
        })
    } else {
        run()
    }
}

impl PalaceDb {
    pub fn open(palace_path: &std::path::Path) -> anyhow::Result<Self> {
        Self::open_collection(palace_path, DEFAULT_COLLECTION_NAME)
    }

    /// Path to the palace directory (used by search strategies that
    /// need direct SQLite access for FTS5 / trigram).
    pub fn path(&self) -> &std::path::Path {
        &self.palace_path
    }

    pub fn open_collection(
        palace_path: &std::path::Path,
        collection_name: &str,
    ) -> anyhow::Result<Self> {
        let collection_name = collection_name.to_string();
        let docs_path = palace_path.join(format!("{}.json", collection_name));

        let documents: HashMap<String, DocumentEntry> = if docs_path.exists() {
            let content = std::fs::read_to_string(&docs_path)?;
            serde_json::from_str(&content)
                .with_context(|| format!("failed to parse collection at {}", docs_path.display()))?
        } else {
            HashMap::new()
        };

        let embedder: Arc<dyn crate::embed::Embedder> =
            Arc::new(crate::embed::NullEmbedder::new(384));

        // Rebuild BM25 index from loaded documents so hybrid_search has data.
        let mut bm25 = bm25::SearchEngineBuilder::with_avgdl(100.0)
            .b(0.3)
            .k1(1.5)
            .build();
        for (id, entry) in &documents {
            bm25.upsert(bm25::Document::new(id.clone(), entry.content.clone()));
        }

        Ok(Self {
            documents,
            palace_path: palace_path.to_path_buf(),
            collection_name,
            coordination: Arc::new(Mutex::new(CoordinationDb::open(palace_path)?)),
            bm25,
            embedder,
            embedding_db: None,
        })
    }

    pub fn coordination(&self) -> std::sync::MutexGuard<'_, CoordinationDb> {
        self.coordination.lock().unwrap()
    }

    pub fn slot_list(&self, project: Option<&str>) -> Result<Vec<MemorySlot>, DbErr> {
        let mut conn = self.coordination.lock().unwrap();
        let query = if project.is_some() {
            "SELECT id, label, content, size_limit, description, pinned, scope, project, created_at, updated_at FROM slots WHERE scope = 'global' OR project = ?1 ORDER BY pinned DESC, label ASC"
        } else {
            "SELECT id, label, content, size_limit, description, pinned, scope, project, created_at, updated_at FROM slots WHERE scope = 'global' ORDER BY pinned DESC, label ASC"
        };
        let mut stmt = conn.prepare(query)?;
        let project_val = project.unwrap_or("");
        let mut rows = if project.is_some() {
            stmt.query(params![project_val])?
        } else {
            stmt.query([])?
        };
        let mut slots = Vec::new();
        while let Some(row) = rows.next()? {
            slots.push(MemorySlot {
                id: row.get(0)?,
                label: row.get(1)?,
                content: row.get(2)?,
                size_limit: row.get(3)?,
                description: row.get(4)?,
                pinned: row.get::<_, i32>(5)? != 0,
                scope: row.get(6)?,
                project: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            });
        }
        Ok(slots)
    }

    pub fn slot_get(&self, label: &str) -> Result<Option<MemorySlot>, DbErr> {
        let mut conn = self.coordination.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, label, content, size_limit, description, pinned, scope, project, created_at, updated_at FROM slots WHERE label = ?1",
        )?;
        let mut rows = stmt.query(params![label])?;
        if let Some(row) = rows.next()? {
            Ok(Some(MemorySlot {
                id: row.get(0)?,
                label: row.get(1)?,
                content: row.get(2)?,
                size_limit: row.get(3)?,
                description: row.get(4)?,
                pinned: row.get::<_, i32>(5)? != 0,
                scope: row.get(6)?,
                project: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn slot_create(&mut self, slot: &MemorySlot) -> Result<(), DbErr> {
        let mut conn = self.coordination.lock().unwrap();
        conn.execute(
            "INSERT INTO slots (id, label, content, size_limit, description, pinned, scope, project, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                slot.id,
                slot.label,
                slot.content,
                slot.size_limit,
                slot.description,
                slot.pinned as i32,
                slot.scope,
                slot.project,
                slot.created_at,
                slot.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn slot_append(&mut self, label: &str, text: &str) -> Result<i32, DbErr> {
        let mut conn = self.coordination.lock().unwrap();
        let mut stmt = conn.prepare("SELECT content, size_limit FROM slots WHERE label = ?1")?;
        let mut rows = stmt.query(params![label])?;
        let (current_content, size_limit): (String, i32) = match rows.next()? {
            Some(row) => (row.get(0)?, row.get(1)?),
            None => return Err(rusqlite::Error::QueryReturnedNoRows),
        };
        let new_content = format!("{}{}", current_content, text);
        if new_content.len() as i32 > size_limit {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE slots SET content = ?1, updated_at = ?2 WHERE label = ?3",
            params![new_content, now, label],
        )?;
        Ok(new_content.len() as i32)
    }

    pub fn slot_replace(&mut self, label: &str, content: &str) -> Result<(), DbErr> {
        let mut conn = self.coordination.lock().unwrap();
        let mut stmt = conn.prepare("SELECT size_limit FROM slots WHERE label = ?1")?;
        let mut rows = stmt.query(params![label])?;
        let size_limit: i32 = match rows.next()? {
            Some(row) => row.get(0)?,
            None => return Err(rusqlite::Error::QueryReturnedNoRows),
        };
        if content.len() as i32 > size_limit {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE slots SET content = ?1, updated_at = ?2 WHERE label = ?3",
            params![content, now, label],
        )?;
        Ok(())
    }

    pub fn slot_delete(&mut self, label: &str) -> Result<(), DbErr> {
        let mut conn = self.coordination.lock().unwrap();
        conn.execute("DELETE FROM slots WHERE label = ?1", params![label])?;
        Ok(())
    }

    pub fn sketch_create(&mut self, sketch: &SketchRecord) -> Result<(), DbErr> {
        let mut conn = self.coordination.lock().unwrap();
        conn.execute(
            "INSERT INTO sketches (id, title, description, steps, project, expires_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                sketch.id,
                sketch.title,
                sketch.description,
                sketch.steps,
                sketch.project,
                sketch.expires_at,
                sketch.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn sketch_get(&self, id: &str) -> Result<Option<SketchRecord>, DbErr> {
        let conn = self.coordination.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, description, steps, project, expires_at, created_at
             FROM sketches WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(SketchRecord {
                id: row.get(0)?,
                title: row.get(1)?,
                description: row.get(2)?,
                steps: row.get(3)?,
                project: row.get(4)?,
                expires_at: row.get(5)?,
                created_at: row.get(6)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn sketch_list(&self, project: Option<&str>) -> Result<Vec<SketchRecord>, DbErr> {
        let conn = self.coordination.lock().unwrap();
        let mut sketches = Vec::new();
        match project {
            Some(p) => {
                let mut stmt = conn.prepare(
                    "SELECT id, title, description, steps, project, expires_at, created_at
                     FROM sketches WHERE project = ?1 ORDER BY created_at DESC",
                )?;
                let rows = stmt.query_map(params![p], |row| {
                    Ok(SketchRecord {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        description: row.get(2)?,
                        steps: row.get(3)?,
                        project: row.get(4)?,
                        expires_at: row.get(5)?,
                        created_at: row.get(6)?,
                    })
                })?;
                for sketch in rows {
                    sketches.push(sketch?);
                }
            }
            None => {
                let mut stmt = conn.prepare(
                    "SELECT id, title, description, steps, project, expires_at, created_at
                     FROM sketches ORDER BY created_at DESC",
                )?;
                let rows = stmt.query_map([], |row| {
                    Ok(SketchRecord {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        description: row.get(2)?,
                        steps: row.get(3)?,
                        project: row.get(4)?,
                        expires_at: row.get(5)?,
                        created_at: row.get(6)?,
                    })
                })?;
                for sketch in rows {
                    sketches.push(sketch?);
                }
            }
        };
        Ok(sketches)
    }

    pub fn sketch_delete(&mut self, id: &str) -> Result<(), DbErr> {
        let mut conn = self.coordination.lock().unwrap();
        conn.execute("DELETE FROM sketches WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn sketch_cleanup_expired(&mut self) -> Result<usize, DbErr> {
        let mut conn = self.coordination.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let count = conn.execute("DELETE FROM sketches WHERE expires_at < ?1", params![now])?;
        Ok(count)
    }

    pub fn crystal_create(&mut self, crystal: &CrystalRecord) -> Result<(), DbErr> {
        let mut conn = self.coordination.lock().unwrap();
        conn.execute(
            "INSERT INTO crystals (id, action_ids, summary, narrative, outcomes, files_affected, lessons, project, session_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                crystal.id,
                crystal.action_ids,
                crystal.summary,
                crystal.narrative,
                crystal.outcomes,
                crystal.files_affected,
                crystal.lessons,
                crystal.project,
                crystal.session_id,
                crystal.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn crystal_list(&self, project: Option<&str>) -> Result<Vec<CrystalRecord>, DbErr> {
        let conn = self.coordination.lock().unwrap();
        let mut crystals = Vec::new();
        match project {
            Some(p) => {
                let mut stmt = conn.prepare(
                    "SELECT id, action_ids, summary, narrative, outcomes, files_affected, lessons, project, session_id, created_at
                     FROM crystals WHERE project = ?1 ORDER BY created_at DESC",
                )?;
                let rows = stmt.query_map(params![p], |row| {
                    Ok(CrystalRecord {
                        id: row.get(0)?,
                        action_ids: row.get(1)?,
                        summary: row.get(2)?,
                        narrative: row.get(3)?,
                        outcomes: row.get(4)?,
                        files_affected: row.get(5)?,
                        lessons: row.get(6)?,
                        project: row.get(7)?,
                        session_id: row.get(8)?,
                        created_at: row.get(9)?,
                    })
                })?;
                for crystal in rows {
                    crystals.push(crystal?);
                }
            }
            None => {
                let mut stmt = conn.prepare(
                    "SELECT id, action_ids, summary, narrative, outcomes, files_affected, lessons, project, session_id, created_at
                     FROM crystals ORDER BY created_at DESC",
                )?;
                let rows = stmt.query_map([], |row| {
                    Ok(CrystalRecord {
                        id: row.get(0)?,
                        action_ids: row.get(1)?,
                        summary: row.get(2)?,
                        narrative: row.get(3)?,
                        outcomes: row.get(4)?,
                        files_affected: row.get(5)?,
                        lessons: row.get(6)?,
                        project: row.get(7)?,
                        session_id: row.get(8)?,
                        created_at: row.get(9)?,
                    })
                })?;
                for crystal in rows {
                    crystals.push(crystal?);
                }
            }
        };
        Ok(crystals)
    }

    pub fn facet_create(&mut self, facet: &FacetRecord) -> Result<(), DbErr> {
        let mut conn = self.coordination.lock().unwrap();
        conn.execute(
            "INSERT INTO facets (id, target_id, target_type, dimension, value, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                facet.id,
                facet.target_id,
                facet.target_type,
                facet.dimension,
                facet.value,
                facet.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn facet_list(
        &self,
        target_id: Option<&str>,
        dimension: Option<&str>,
    ) -> Result<Vec<FacetRecord>, DbErr> {
        let conn = self.coordination.lock().unwrap();
        let mut facets = Vec::new();
        match (target_id, dimension) {
            (Some(tid), Some(dim)) => {
                let mut stmt = conn.prepare(
                    "SELECT id, target_id, target_type, dimension, value, created_at
                     FROM facets WHERE target_id = ?1 AND dimension = ?2 ORDER BY created_at DESC",
                )?;
                let rows = stmt.query_map(params![tid, dim], |row| {
                    Ok(FacetRecord {
                        id: row.get(0)?,
                        target_id: row.get(1)?,
                        target_type: row.get(2)?,
                        dimension: row.get(3)?,
                        value: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                })?;
                for facet in rows {
                    facets.push(facet?);
                }
            }
            (Some(tid), None) => {
                let mut stmt = conn.prepare(
                    "SELECT id, target_id, target_type, dimension, value, created_at
                     FROM facets WHERE target_id = ?1 ORDER BY created_at DESC",
                )?;
                let rows = stmt.query_map(params![tid], |row| {
                    Ok(FacetRecord {
                        id: row.get(0)?,
                        target_id: row.get(1)?,
                        target_type: row.get(2)?,
                        dimension: row.get(3)?,
                        value: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                })?;
                for facet in rows {
                    facets.push(facet?);
                }
            }
            (None, Some(dim)) => {
                let mut stmt = conn.prepare(
                    "SELECT id, target_id, target_type, dimension, value, created_at
                     FROM facets WHERE dimension = ?1 ORDER BY created_at DESC",
                )?;
                let rows = stmt.query_map(params![dim], |row| {
                    Ok(FacetRecord {
                        id: row.get(0)?,
                        target_id: row.get(1)?,
                        target_type: row.get(2)?,
                        dimension: row.get(3)?,
                        value: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                })?;
                for facet in rows {
                    facets.push(facet?);
                }
            }
            (None, None) => {
                let mut stmt = conn.prepare(
                    "SELECT id, target_id, target_type, dimension, value, created_at
                     FROM facets ORDER BY created_at DESC",
                )?;
                let rows = stmt.query_map([], |row| {
                    Ok(FacetRecord {
                        id: row.get(0)?,
                        target_id: row.get(1)?,
                        target_type: row.get(2)?,
                        dimension: row.get(3)?,
                        value: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                })?;
                for facet in rows {
                    facets.push(facet?);
                }
            }
        };
        Ok(facets)
    }

    pub fn facet_delete(&mut self, id: &str) -> Result<(), DbErr> {
        let mut conn = self.coordination.lock().unwrap();
        conn.execute("DELETE FROM facets WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn lesson_create(&mut self, lesson: &LessonRecord) -> Result<(), DbErr> {
        let mut conn = self.coordination.lock().unwrap();
        conn.execute(
            "INSERT INTO lessons (id, content, context, confidence, project, tags, reinforced_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                lesson.id,
                lesson.content,
                lesson.context,
                lesson.confidence,
                lesson.project,
                lesson.tags,
                lesson.reinforced_at,
                lesson.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn lesson_list(
        &self,
        project: Option<&str>,
        min_confidence: Option<f64>,
    ) -> Result<Vec<LessonRecord>, DbErr> {
        let conn = self.coordination.lock().unwrap();
        let mut lessons = Vec::new();
        match (project, min_confidence) {
            (Some(p), Some(conf)) => {
                let mut stmt = conn.prepare(
                    "SELECT id, content, context, confidence, project, tags, reinforced_at, created_at
                     FROM lessons WHERE project = ?1 AND confidence >= ?2 ORDER BY confidence DESC, created_at DESC",
                )?;
                let rows = stmt.query_map(params![p, conf], |row| {
                    Ok(LessonRecord {
                        id: row.get(0)?,
                        content: row.get(1)?,
                        context: row.get(2)?,
                        confidence: row.get(3)?,
                        project: row.get(4)?,
                        tags: row.get(5)?,
                        reinforced_at: row.get(6)?,
                        created_at: row.get(7)?,
                    })
                })?;
                for lesson in rows {
                    lessons.push(lesson?);
                }
            }
            (Some(p), None) => {
                let mut stmt = conn.prepare(
                    "SELECT id, content, context, confidence, project, tags, reinforced_at, created_at
                     FROM lessons WHERE project = ?1 ORDER BY confidence DESC, created_at DESC",
                )?;
                let rows = stmt.query_map(params![p], |row| {
                    Ok(LessonRecord {
                        id: row.get(0)?,
                        content: row.get(1)?,
                        context: row.get(2)?,
                        confidence: row.get(3)?,
                        project: row.get(4)?,
                        tags: row.get(5)?,
                        reinforced_at: row.get(6)?,
                        created_at: row.get(7)?,
                    })
                })?;
                for lesson in rows {
                    lessons.push(lesson?);
                }
            }
            (None, Some(conf)) => {
                let mut stmt = conn.prepare(
                    "SELECT id, content, context, confidence, project, tags, reinforced_at, created_at
                     FROM lessons WHERE confidence >= ?1 ORDER BY confidence DESC, created_at DESC",
                )?;
                let rows = stmt.query_map(params![conf], |row| {
                    Ok(LessonRecord {
                        id: row.get(0)?,
                        content: row.get(1)?,
                        context: row.get(2)?,
                        confidence: row.get(3)?,
                        project: row.get(4)?,
                        tags: row.get(5)?,
                        reinforced_at: row.get(6)?,
                        created_at: row.get(7)?,
                    })
                })?;
                for lesson in rows {
                    lessons.push(lesson?);
                }
            }
            (None, None) => {
                let mut stmt = conn.prepare(
                    "SELECT id, content, context, confidence, project, tags, reinforced_at, created_at
                     FROM lessons ORDER BY confidence DESC, created_at DESC",
                )?;
                let rows = stmt.query_map([], |row| {
                    Ok(LessonRecord {
                        id: row.get(0)?,
                        content: row.get(1)?,
                        context: row.get(2)?,
                        confidence: row.get(3)?,
                        project: row.get(4)?,
                        tags: row.get(5)?,
                        reinforced_at: row.get(6)?,
                        created_at: row.get(7)?,
                    })
                })?;
                for lesson in rows {
                    lessons.push(lesson?);
                }
            }
        };
        Ok(lessons)
    }

    pub fn lesson_reinforce(&mut self, id: &str) -> Result<(), DbErr> {
        let mut conn = self.coordination.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE lessons SET reinforced_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }

    /// Set a lesson's confidence to an explicit value (e.g. after Ebbinghaus decay).
    /// Returns the number of rows updated.
    pub fn lesson_set_confidence(&mut self, id: &str, confidence: f64) -> Result<usize, DbErr> {
        let mut conn = self.coordination.lock().unwrap();
        let n = conn.execute(
            "UPDATE lessons SET confidence = ?1 WHERE id = ?2",
            params![confidence, id],
        )?;
        Ok(n)
    }

    pub fn insight_create(&mut self, insight: &InsightRecord) -> Result<(), DbErr> {
        let mut conn = self.coordination.lock().unwrap();
        conn.execute(
            "INSERT INTO insights (id, content, confidence, project, cluster_id, reinforced_count, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                insight.id,
                insight.content,
                insight.confidence,
                insight.project,
                insight.cluster_id,
                insight.reinforced_count,
                insight.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn insight_list(
        &self,
        project: Option<&str>,
        min_confidence: Option<f64>,
    ) -> Result<Vec<InsightRecord>, DbErr> {
        let conn = self.coordination.lock().unwrap();
        let mut insights = Vec::new();
        let sql = match (project, min_confidence) {
            (Some(_), Some(_)) => "SELECT id, content, confidence, project, cluster_id, reinforced_count, created_at FROM insights WHERE project = ?1 AND confidence >= ?2 ORDER BY confidence DESC, created_at DESC",
            (Some(_), None) => "SELECT id, content, confidence, project, cluster_id, reinforced_count, created_at FROM insights WHERE project = ?1 ORDER BY confidence DESC, created_at DESC",
            (None, Some(_)) => "SELECT id, content, confidence, project, cluster_id, reinforced_count, created_at FROM insights WHERE confidence >= ?1 ORDER BY confidence DESC, created_at DESC",
            (None, None) => "SELECT id, content, confidence, project, cluster_id, reinforced_count, created_at FROM insights ORDER BY confidence DESC, created_at DESC",
        };
        let mut stmt = conn.prepare(sql)?;
        let mut rows = match (project, min_confidence) {
            (Some(p), Some(conf)) => stmt.query(params![p, conf])?,
            (Some(p), None) => stmt.query(params![p])?,
            (None, Some(conf)) => stmt.query(params![conf])?,
            (None, None) => stmt.query([])?,
        };
        while let Some(row) = rows.next()? {
            insights.push(InsightRecord {
                id: row.get(0)?,
                content: row.get(1)?,
                confidence: row.get(2)?,
                project: row.get(3)?,
                cluster_id: row.get(4)?,
                reinforced_count: row.get(5)?,
                created_at: row.get(6)?,
            });
        }
        Ok(insights)
    }

    pub fn insight_reinforce(&mut self, id: &str) -> Result<(), DbErr> {
        let mut conn = self.coordination.lock().unwrap();
        conn.execute(
            "UPDATE insights SET reinforced_count = reinforced_count + 1 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    /// Embedder-aware open path (mp-016 / ADR-8).
    ///
    /// Performs the same work as [`Self::open`] **plus** the
    /// `embedding.json` manifest dance:
    ///
    /// * If a manifest exists on disk, validate it against `embedder`'s
    ///   `dim()` and `fingerprint()`. A mismatch returns
    ///   [`crate::embed::ManifestMismatch`] wrapped in
    ///   [`crate::error::MempalaceError::ManifestMismatch`] whose
    ///   message points at `mpr migrate --re-embed`.
    /// * If absent and the palace has zero drawers (fresh install),
    ///   write a manifest from `embedder`.
    /// * If absent but drawers already exist (legacy palace from before
    ///   mp-015 landed), emit a `tracing::warn!` and write a
    ///   best-effort manifest. We can't validate the *existing* vectors
    ///   against this manifest after the fact, but we can at least
    ///   ensure subsequent opens are checked.
    ///
    /// Override: `MEMPALACE_SKIP_MANIFEST_CHECK=1` skips validation.
    /// This is a deliberate test-and-migration backdoor; production
    /// code paths must not set it. The override emits a `warn!` so it
    /// shows up in logs.
    ///
    /// `model_name` is supplied by the caller because [`Embedder`]
    /// deliberately keeps the human-readable model name out of its
    /// surface (different backends have different concepts of a
    /// "name"). Used only when *writing* a new manifest.
    pub fn open_with_embedder(
        palace_path: &std::path::Path,
        embedder: Arc<dyn crate::embed::Embedder>,
        model_name: &str,
    ) -> anyhow::Result<Self> {
        Self::open_collection_with_embedder(
            palace_path,
            DEFAULT_COLLECTION_NAME,
            embedder,
            model_name,
        )
    }

    /// Collection-aware variant of [`Self::open_with_embedder`]
    /// (mp-016 / ADR-8). See that method's docs for the full contract.
    pub fn open_collection_with_embedder(
        palace_path: &std::path::Path,
        collection_name: &str,
        embedder: Arc<dyn crate::embed::Embedder>,
        model_name: &str,
    ) -> anyhow::Result<Self> {
        let collection_name = collection_name.to_string();
        let docs_path = palace_path.join(format!("{}.json", collection_name));

        let documents: HashMap<String, DocumentEntry> = if docs_path.exists() {
            let content = std::fs::read_to_string(&docs_path)?;
            serde_json::from_str(&content)
                .with_context(|| format!("failed to parse collection at {}", docs_path.display()))?
        } else {
            HashMap::new()
        };

        let embedding_db = EmbeddingDb::with_embedder(embedder.clone())?;
        validate_or_write_manifest(palace_path, embedder.as_ref(), model_name, documents.len())?;

        let mut db = Self {
            documents,
            palace_path: palace_path.to_path_buf(),
            collection_name,
            coordination: Arc::new(Mutex::new(CoordinationDb::open(palace_path)?)),
            bm25: bm25::SearchEngineBuilder::with_avgdl(100.0)
                .b(0.3)
                .k1(1.5)
                .build(),
            embedder,
            embedding_db: Some(embedding_db),
        };

        // Try loading cached embeddings first (fast), fall back to sync_embeddings (slow)
        let cache_path = palace_path.join("embedding_cache");
        match EmbeddingDb::load_cache(&cache_path, db.embedder.clone()) {
            Ok(cached) => {
                tracing::debug!("loaded cached embeddings ({} vectors)", cached.len());
                db.embedding_db = Some(cached);
            }
            Err(_) => {
                tracing::debug!("no cached embeddings, computing from scratch");
                let _ = db.sync_embeddings();
                // Save cache for next reopen
                if let Some(ref emb) = db.embedding_db {
                    let _ = emb.save_cache(&cache_path);
                }
            }
        }

        // Rebuild BM25 index from loaded documents so hybrid_search
        // has a populated BM25 stream alongside vector + graph.
        for (id, entry) in &db.documents {
            db.bm25
                .upsert(bm25::Document::new(id.clone(), entry.content.clone()));
        }

        Ok(db)
    }

    pub async fn query(
        &self,
        query_text: &str,
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
    ) -> anyhow::Result<Vec<QueryResult>> {
        self.query_sync(query_text, wing, room, n_results)
    }

    pub fn query_sync(
        &self,
        query_text: &str,
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
    ) -> anyhow::Result<Vec<QueryResult>> {
        self.query_sync_with_filter(query_text, wing, room, n_results, None)
    }

    pub fn query_sync_with_filter(
        &self,
        query_text: &str,
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
        metadata_filter: Option<&std::collections::HashMap<String, String>>,
    ) -> anyhow::Result<Vec<QueryResult>> {
        let query_lower = query_text.to_lowercase();

        let mut results: Vec<(String, f64, &DocumentEntry)> = self
            .documents
            .iter()
            .filter_map(|(id, entry)| {
                if let Some(w) = wing {
                    let entry_wing = entry
                        .metadata
                        .get("wing")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if entry_wing != w {
                        return None;
                    }
                }
                if let Some(r) = room {
                    let entry_room = entry
                        .metadata
                        .get("room")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if entry_room != r {
                        return None;
                    }
                }

                // Apply custom metadata filter if provided
                if let Some(filter) = metadata_filter {
                    for (key, expected_value) in filter {
                        let entry_value = entry
                            .metadata
                            .get(key)
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if entry_value != *expected_value {
                            return None;
                        }
                    }
                }

                let similarity = naive_similarity(&query_lower, &entry.content.to_lowercase());
                if similarity > 0.05 {
                    Some((id.clone(), similarity, entry))
                } else {
                    None
                }
            })
            .collect();

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(n_results);

        let query_results: Vec<QueryResult> = results
            .into_iter()
            .map(|(id, similarity, entry)| {
                let mut metadata = entry.metadata.clone();
                metadata.insert("distance".to_string(), serde_json::json!(1.0 - similarity));

                QueryResult {
                    ids: vec![id],
                    documents: vec![entry.content.clone()],
                    distances: vec![1.0 - similarity],
                    metadatas: vec![metadata],
                }
            })
            .collect();

        Ok(query_results)
    }

    /// Hybrid search: BM25 + naive similarity + Graph via RRF fusion.
    ///
    /// Returns results sorted by combined RRF score.
    pub fn hybrid_search(
        &self,
        query_text: &str,
        limit: usize,
        wing: Option<&str>,
        room: Option<&str>,
    ) -> anyhow::Result<Vec<QueryResult>> {
        use crate::search::diversify::diversify_by_session;
        use crate::search::rrf::{fuse_results, RrfConfig, SearchStream, StreamResult};
        use crate::search::synonyms::expand_query as expand_query_synonyms;

        let over_fetch = (limit * 3).min(300);

        // Synonym expansion: append synonym tokens to the query so BM25
        // and graph search surface documents that match via canonical
        // abbreviations (e.g. `k8s` matches `kubernetes` rows). The
        // expanded query is used for lexical streams (BM25 + graph);
        // the vector stream stays semantic and still uses the raw
        // query_text so the embedder doesn't see noisy expansions.
        // Per `mempalace/src/state/search-index.ts:98` the BM25
        // weight for synonym-matched docs is 0.7 (vs 1.0 for direct
        // matches), wired through `RrfConfig::with_synonyms()` below.
        let query_tokens: Vec<&str> = query_text.split_whitespace().collect();
        let expanded_tokens = expand_query_synonyms(&query_tokens);
        let expanded_query = expanded_tokens.join(" ");
        // Graph search takes the first N tokens (originals + expanded)
        // — bumping from 5 (original) to 10 covers the most common
        // synonym groups (k8s, pg, ts, py, etc.) in a single pass.
        let graph_tokens: Vec<&str> = expanded_tokens
            .iter()
            .take(10)
            .map(String::as_str)
            .collect();

        // Helper: check if a document entry passes wing/room filter
        let passes_filter = |entry: &DocumentEntry| {
            if let Some(w) = wing {
                let entry_wing = entry
                    .metadata
                    .get("wing")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if entry_wing != w {
                    return false;
                }
            }
            if let Some(r) = room {
                let entry_room = entry
                    .metadata
                    .get("room")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if entry_room != r {
                    return false;
                }
            }
            true
        };

        let bm25_results: Vec<StreamResult> = self
            .bm25
            .search(&expanded_query, over_fetch)
            .into_iter()
            .enumerate()
            .filter_map(|(rank, result)| {
                let entry = self.documents.get(&result.document.id)?;
                if !passes_filter(entry) {
                    return None;
                }
                Some(StreamResult {
                    id: result.document.id,
                    rank,
                    stream: SearchStream::Bm25,
                })
            })
            .collect();

        let query_lower = query_text.to_lowercase();
        // Vector search using real embeddings (async embedder in sync context).
        // Only performed when an EmbeddingDb has been populated (either by
        // open_with_embedder or by lazy construction on first search).
        let vector_results: Vec<StreamResult> = {
            if let Some(ref embedding_db) = self.embedding_db {
                let (embedder, q) = (self.embedder.clone(), query_text.to_string());
                match run_off_runtime(move || async move { embedder.embed(&q).await }) {
                    Ok(query_embedding) => {
                        let normalized_query = normalize_embedding(&query_embedding);
                        match embedding_db.query_by_vector(&normalized_query, over_fetch) {
                            Ok(results) => results
                                .into_iter()
                                .filter_map(|(dist, idx)| {
                                    let doc_id = embedding_db.id_at(idx)?;
                                    let entry = self.documents.get(&doc_id)?;
                                    if !passes_filter(entry) {
                                        return None;
                                    }
                                    Some(StreamResult {
                                        id: doc_id,
                                        rank: idx,
                                        stream: SearchStream::Vector,
                                    })
                                })
                                .collect(),
                            Err(_) => vec![],
                        }
                    }
                    Err(e) => {
                        tracing::debug!("embedding failed: {}", e);
                        vec![]
                    }
                }
            } else {
                vec![]
            }
        };

        let mut graph_results: Vec<StreamResult> = vec![];
        if let Ok(kg) = crate::knowledge_graph::KnowledgeGraph::open(
            &self.palace_path.join("knowledge_graph.db"),
        ) {
            let query_words: Vec<&str> = graph_tokens.clone();
            for word in query_words {
                if let Ok(triples) = kg.query_entity(word, None, None, "both") {
                    for (rank, triple) in triples.iter().enumerate() {
                        // Filter graph results by wing/room too
                        if let Some(entry) = self.documents.get(&triple.subject) {
                            if passes_filter(entry) {
                                graph_results.push(StreamResult {
                                    id: triple.subject.clone(),
                                    rank,
                                    stream: SearchStream::Graph,
                                });
                            }
                        }
                        if graph_results.len() >= over_fetch {
                            break;
                        }
                    }
                }
            }
        }

        let config = RrfConfig::with_synonyms();
        let fused = fuse_results(&bm25_results, &vector_results, &graph_results, &config);

        let diversified: Vec<_> = fused
            .into_iter()
            .take(over_fetch)
            .filter_map(|fr| {
                let doc = self.documents.get(&fr.id)?;
                let session_id = doc
                    .metadata
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default")
                    .to_string();
                Some(crate::search::diversify::DiversifiableResult {
                    id: fr.id.clone(),
                    session_id,
                    score: fr.combined_score,
                })
            })
            .collect();

        let diversified = diversify_by_session(&diversified, limit, 3);

        let results: Vec<QueryResult> = diversified
            .into_iter()
            .filter_map(|dr| {
                let entry = self.documents.get(&dr.id)?;
                let mut metadata = entry.metadata.clone();
                metadata.insert("combined_score".to_string(), serde_json::json!(dr.score));
                Some(QueryResult {
                    ids: vec![dr.id],
                    documents: vec![entry.content.clone()],
                    distances: vec![dr.score],
                    metadatas: vec![metadata],
                })
            })
            .collect();

        Ok(results)
    }

    pub fn add(
        &mut self,
        documents: &[(&str, &str)],
        metadata: &[&[(&str, &str)]],
    ) -> anyhow::Result<()> {
        for ((id, content), meta) in documents.iter().zip(metadata.iter()) {
            let meta_map: HashMap<String, serde_json::Value> = meta
                .iter()
                .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
                .collect();

            // Privacy filter (mp-031, ADR-12) — strip API keys, JWTs,
            // private keys, and other well-known secret patterns from the
            // verbatim drawer body before it lands on disk. Wing/room slugs
            // are structural and never run through the redactor; only the
            // user-text `content` field is processed.
            let redacted = crate::privacy::redact(content).redacted_text;

            self.documents.insert(
                id.to_string(),
                DocumentEntry {
                    content: redacted.clone(),
                    metadata: meta_map,
                },
            );

            // Index document in BM25
            self.bm25
                .upsert(bm25::Document::new(id.to_string(), redacted.clone()));
        }

        // Don't auto-save on every add - caller should call flush() when done batching
        Ok(())
    }

    /// Sync all documents to embedding index. Call after loading or batch adding.
    /// Builds a fresh embedding index over every stored document and installs it
    /// as `self.embedding_db`. Runs safely whether or not a tokio runtime is
    /// already active; on embedding failure it logs and leaves the index as-is.
    pub fn sync_embeddings(&mut self) -> anyhow::Result<()> {
        let items: Vec<(String, String)> = self
            .documents
            .iter()
            .map(|(id, entry)| (id.clone(), entry.content.clone()))
            .collect();
        if items.is_empty() {
            return Ok(());
        }
        let embedder = self.embedder.clone();
        let built = run_off_runtime(move || async move {
            let mut db = EmbeddingDb::with_embedder(embedder)?;
            db.add_batch(&items).await?;
            anyhow::Ok(db)
        });
        match built {
            Ok(db) => self.embedding_db = Some(db),
            Err(e) => tracing::debug!("embedding sync skipped: {}", e),
        }
        Ok(())
    }

    pub fn upsert_documents(
        &mut self,
        documents: &[(String, String, HashMap<String, serde_json::Value>)],
    ) -> anyhow::Result<()> {
        for (id, content, metadata) in documents {
            // Privacy filter (mp-031, ADR-12) — same as `add()`.
            let redacted = crate::privacy::redact(content).redacted_text;

            self.documents.insert(
                id.clone(),
                DocumentEntry {
                    content: redacted.clone(),
                    metadata: metadata.clone(),
                },
            );
            self.bm25
                .upsert(bm25::Document::new(id.clone(), redacted.clone()));
        }

        Ok(())
    }

    pub fn delete_id(&mut self, id: &str) -> anyhow::Result<bool> {
        let removed = self.documents.remove(id).is_some();
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    pub fn file_already_mined(&self, source_file: &str, check_mtime: bool) -> bool {
        self.file_already_mined_with_mode(source_file, check_mtime, None)
    }

    /// Extract-mode-aware variant of [`Self::file_already_mined`] (#1505).
    ///
    /// When `extract_mode` is `Some`, only drawers whose stored
    /// `extract_mode` metadata matches the argument are considered. Legacy
    /// drawers (no `extract_mode` field) are treated as `exchange`-mode so
    /// pre-#1505 palaces still classify correctly.
    pub fn file_already_mined_with_mode(
        &self,
        source_file: &str,
        check_mtime: bool,
        extract_mode: Option<&str>,
    ) -> bool {
        let Some(entry) = self.documents.values().find(|entry| {
            let same_source =
                entry.metadata.get("source_file").and_then(|v| v.as_str()) == Some(source_file);
            if !same_source {
                return false;
            }
            match extract_mode {
                None => true,
                Some(want) => {
                    let stored = entry.metadata.get("extract_mode").and_then(|v| v.as_str());
                    match stored {
                        Some(value) => value == want,
                        // Legacy: unfielded rows are treated as exchange.
                        None => want == "exchange",
                    }
                }
            }
        }) else {
            return false;
        };

        // Pre-v2 drawers have no version field — treat them as stale.
        // Returns false so the file gets re-mined with the new schema.
        let stored_version = entry
            .metadata
            .get("normalize_version")
            .and_then(|v| {
                v.as_i64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(1);
        if stored_version < crate::constants::NORMALIZE_VERSION as i64 {
            return false;
        }

        if !check_mtime {
            return true;
        }

        let Some(stored_mtime) = entry
            .metadata
            .get("source_mtime")
            .and_then(|v| v.as_str())
            .and_then(|v| v.parse::<f64>().ok())
        else {
            return false;
        };

        let Ok(metadata) = std::fs::metadata(source_file) else {
            return false;
        };
        let Ok(modified) = metadata.modified() else {
            return false;
        };
        let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) else {
            return false;
        };

        (duration.as_secs_f64() - stored_mtime).abs() < f64::EPSILON
    }

    pub fn flush(&mut self) -> anyhow::Result<()> {
        self.save()
    }

    pub fn complete_test_setup(&mut self) -> anyhow::Result<()> {
        self.flush()
    }

    fn save(&self) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.palace_path)?;

        let docs_path = self
            .palace_path
            .join(format!("{}.json", self.collection_name));
        let content = serde_json::to_string_pretty(&self.documents)?;
        std::fs::write(docs_path, content)?;

        Ok(())
    }

    pub(crate) fn _get_document(&self, id: &str) -> Option<&DocumentEntry> {
        self.documents.get(id)
    }

    /// Get metadata for a document by ID.
    pub fn get_document_metadata(&self, id: &str) -> Option<&HashMap<String, serde_json::Value>> {
        self.documents.get(id).map(|e| &e.metadata)
    }

    /// Get documents by their IDs. Returns only the IDs that exist.
    pub fn get_documents(&self, ids: &[String]) -> Vec<String> {
        ids.iter()
            .filter(|id| self.documents.contains_key(id.as_str()))
            .cloned()
            .collect()
    }

    /// Get full document entries (id, content, metadata) for the given IDs.
    /// Returns only entries that exist.
    pub fn get_documents_with_metadata(
        &self,
        ids: &[String],
    ) -> Vec<(String, String, HashMap<String, serde_json::Value>)> {
        ids.iter()
            .filter_map(|id| {
                self.documents
                    .get(id)
                    .map(|entry| (id.clone(), entry.content.clone(), entry.metadata.clone()))
            })
            .collect()
    }

    /// Get all documents that have a matching session_id in their metadata.
    /// Returns vector of (id, content, metadata) tuples.
    pub fn get_documents_by_session(
        &self,
        session_id: &str,
    ) -> Vec<(String, String, HashMap<String, serde_json::Value>)> {
        self.documents
            .iter()
            .filter(|(_, entry)| {
                entry.metadata.get("session_id").and_then(|v| v.as_str()) == Some(session_id)
            })
            .map(|(id, entry)| (id.clone(), entry.content.clone(), entry.metadata.clone()))
            .collect()
    }

    pub fn count(&self) -> usize {
        self.documents.len()
    }

    /// Get all documents, optionally filtered by wing and/or room.
    /// Returns results sorted by importance (from metadata or distance).
    pub fn get_all(
        &self,
        wing: Option<&str>,
        room: Option<&str>,
        limit: usize,
    ) -> Vec<QueryResult> {
        let mut entries: Vec<(&String, &DocumentEntry)> = self
            .documents
            .iter()
            .filter(|(_, entry)| {
                if let Some(w) = wing {
                    let entry_wing = entry
                        .metadata
                        .get("wing")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if entry_wing != w {
                        return false;
                    }
                }
                if let Some(r) = room {
                    let entry_room = entry
                        .metadata
                        .get("room")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if entry_room != r {
                        return false;
                    }
                }
                true
            })
            .collect();

        // Sort by importance metadata if available, otherwise by order added
        entries.sort_by(|(id_a, entry_a), (id_b, entry_b)| {
            let imp_a = entry_a
                .metadata
                .get("importance")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.5);
            let imp_b = entry_b
                .metadata
                .get("importance")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.5);
            imp_b
                .partial_cmp(&imp_a)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| id_a.cmp(id_b))
        });

        entries.truncate(limit);

        let query_results: Vec<QueryResult> = entries
            .into_iter()
            .map(|(id, entry)| QueryResult {
                ids: vec![id.clone()],
                documents: vec![entry.content.clone()],
                distances: vec![0.0],
                metadatas: vec![entry.metadata.clone()],
            })
            .collect();

        query_results
    }

    /// Get memories as typed Memory structs from documents.
    /// Falls back to constructing minimal Memory entries when metadata is incomplete.
    pub fn get_memories(&self, wing: Option<&str>, limit: usize) -> Vec<crate::types::Memory> {
        let results = self.get_all(wing, None, limit);
        results
            .into_iter()
            .flat_map(|qr| {
                qr.ids
                    .into_iter()
                    .zip(qr.documents.into_iter())
                    .zip(qr.metadatas.into_iter())
                    .filter_map(|((id, content), metadata)| {
                        let title = metadata
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or(&id)
                            .to_string();
                        let created_at = metadata
                            .get("created_at")
                            .and_then(|v| v.as_str())
                            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                            .map(|dt| dt.with_timezone(&chrono::Utc))
                            .unwrap_or_else(chrono::Utc::now);
                        let updated_at = metadata
                            .get("updated_at")
                            .and_then(|v| v.as_str())
                            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                            .map(|dt| dt.with_timezone(&chrono::Utc))
                            .unwrap_or(created_at);
                        let memory_type = metadata
                            .get("memory_type")
                            .and_then(|v| v.as_str())
                            .and_then(|s| serde_json::from_str::<crate::types::MemoryType>(s).ok())
                            .unwrap_or(crate::types::MemoryType::Semantic);
                        let concepts = metadata
                            .get("concepts")
                            .and_then(|v| v.as_str())
                            .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
                            .unwrap_or_default();
                        let files = metadata
                            .get("files")
                            .and_then(|v| v.as_str())
                            .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
                            .unwrap_or_default();
                        let session_ids = metadata
                            .get("session_ids")
                            .and_then(|v| v.as_str())
                            .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
                            .unwrap_or_default();
                        let strength = metadata
                            .get("strength")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.5);
                        let version = metadata
                            .get("version")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(1) as u32;
                        let parent_id = metadata
                            .get("parent_id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let supersedes = metadata
                            .get("supersedes")
                            .and_then(|v| v.as_str())
                            .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
                            .unwrap_or_default();
                        let related_ids = metadata
                            .get("related_ids")
                            .and_then(|v| v.as_str())
                            .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
                            .unwrap_or_default();
                        let source_observation_ids = metadata
                            .get("source_observation_ids")
                            .and_then(|v| v.as_str())
                            .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
                            .unwrap_or_default();
                        let is_latest = metadata
                            .get("is_latest")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true);
                        let forget_after = metadata
                            .get("forget_after")
                            .and_then(|v| v.as_str())
                            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                            .map(|dt| dt.with_timezone(&chrono::Utc));
                        let image_ref = metadata
                            .get("image_ref")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let agent_id = metadata
                            .get("agent_id")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let project = metadata
                            .get("project")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();

                        Some(crate::types::Memory {
                            id,
                            created_at,
                            updated_at,
                            memory_type,
                            title,
                            content,
                            concepts,
                            files,
                            session_ids,
                            strength,
                            version,
                            parent_id,
                            supersedes,
                            related_ids,
                            source_observation_ids,
                            is_latest,
                            forget_after,
                            image_ref,
                            agent_id,
                            project,
                        })
                    })
            })
            .collect()
    }

    /// Compute synonymy edges between rooms (mp-082).
    ///
    /// Groups drawers by (wing, room), computes word-overlap similarity
    /// between room pairs within each wing, and returns pairs with
    /// similarity > 0.85 (a text-proxy for cosine embedding similarity).
    ///
    /// Returns `(room_a, room_b, wing, similarity)` for each synonymy edge.
    pub fn compute_synonymy_edges(&self, threshold: f64) -> Vec<(String, String, String, f64)> {
        let mut by_room: std::collections::HashMap<(String, String), Vec<&str>> =
            std::collections::HashMap::new();
        for entry in self.documents.values() {
            let wing = entry
                .metadata
                .get("wing")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let room = entry
                .metadata
                .get("room")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if wing.is_empty() || room.is_empty() {
                continue;
            }
            let key = (wing.to_string(), room.to_string());
            by_room.entry(key).or_default().push(&entry.content);
        }

        let mut edges: Vec<(String, String, String, f64)> = Vec::new();
        let mut room_names: Vec<(String, String)> = by_room.keys().cloned().collect();
        room_names.sort();

        for (i, (wing_a, room_a)) in room_names.iter().enumerate() {
            for (wing_b, room_b) in room_names.iter().skip(i + 1) {
                if wing_a != wing_b {
                    continue;
                }
                if room_a == room_b {
                    continue;
                }
                let texts_a = by_room.get(&(wing_a.clone(), room_a.clone())).unwrap();
                let texts_b = by_room.get(&(wing_b.clone(), room_b.clone())).unwrap();
                let sim = pairwise_room_similarity(texts_a, texts_b);
                if sim > threshold {
                    edges.push((room_a.clone(), room_b.clone(), wing_a.clone(), sim));
                }
            }
        }
        edges.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap());
        edges
    }

    pub fn add_drawer(
        &mut self,
        id: &str,
        content: &str,
        metadata: &[(&str, &str)],
    ) -> anyhow::Result<Option<String>> {
        self.add_drawer_with_dedup(&dedup_window(), id, content, metadata)
    }

    /// Variant of [`PalaceDb::add_drawer`] that uses an explicit dedup
    /// instance. Lets call sites (and unit tests) inject scoped state
    /// instead of relying on the process-global window.
    pub fn add_drawer_with_dedup(
        &mut self,
        dedup: &WindowedDedup,
        id: &str,
        content: &str,
        metadata: &[(&str, &str)],
    ) -> anyhow::Result<Option<String>> {
        match dedup.check_and_record(content) {
            DedupVerdict::Duplicate => {
                let hash = crate::dedup_window::hash_normalized(content);
                tracing::debug!(
                    target: "mempalace::dedup",
                    drawer_id = %id,
                    sha256 = %hex::encode(hash),
                    "dedup skipped"
                );
                Ok(None)
            }
            DedupVerdict::Fresh => {
                self.add(&[(id, content)], &[metadata])?;
                Ok(Some(id.to_string()))
            }
        }
    }
}

impl EmbeddingDb {
    /// Construct with an embedder already loaded.
    /// The embedder's `dim()` is used to size the vector storage.
    pub fn with_embedder(embedder: Arc<dyn crate::embed::Embedder>) -> anyhow::Result<Self> {
        let dim = embedder.dim();
        let hnsw = embedvec::HnswIndex::new(16, 200, embedvec::Distance::Cosine);
        let storage = embedvec::VectorStorage::new(dim, embedvec::Quantization::None);
        Ok(Self {
            embedder,
            hnsw,
            documents: Vec::new(),
            storage,
        })
    }

    pub async fn add(&mut self, id: &str, text: &str) -> anyhow::Result<usize> {
        let embedding = self.embed(text).await?;
        let idx = self.documents.len();
        self.documents.push((id.to_string(), text.to_string()));
        self.storage.add(&embedding, None)?;
        self.hnsw.insert(idx, &embedding, &self.storage, None)?;
        Ok(idx).inspect(|_| {
            #[cfg(feature = "telemetry")]
            crate::telemetry::counter!("mempalace_insert_total", "status" => "success")
                .increment(1);
        })
    }

    pub async fn add_batch(&mut self, items: &[(String, String)]) -> anyhow::Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let texts: Vec<&str> = items.iter().map(|(_, t)| t.as_str()).collect();
        let embeddings = self.embedder.embed_batch(&texts).await?;
        let start_idx = self.documents.len();
        for (i, (id, text)) in items.iter().enumerate() {
            self.documents.push((id.clone(), text.clone()));
            // Normalize embeddings before storing
            let normalized = normalize_embedding(&embeddings[i]);
            self.storage.add(&normalized, None)?;
            self.hnsw
                .insert(start_idx + i, &normalized, &self.storage, None)?;
        }
        Ok(())
    }

    pub async fn query(
        &self,
        query_text: &str,
        n_results: usize,
    ) -> anyhow::Result<Vec<(f32, usize)>> {
        let query_embedding = self.embed(query_text).await?;
        let normalized_query = normalize_embedding(&query_embedding);
        let results = self
            .hnsw
            .search(&normalized_query, n_results, 1024, &self.storage, None)?;
        Ok(results.into_iter().map(|(id, dist)| (dist, id)).collect())
    }

    pub async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        #[cfg(feature = "telemetry")]
        let _telemetry_start = std::time::Instant::now();
        let embedding = self.embedder.embed(text).await?;
        #[cfg(feature = "telemetry")]
        {
            crate::telemetry::histogram!("mempalace_embed_latency_ms")
                .record(_telemetry_start.elapsed().as_secs_f64() * 1000.0);
        }
        Ok(embedding)
    }

    /// Search using a pre-computed embedding vector (already normalized).
    /// Returns `(distance, index)` pairs in ascending distance order.
    pub fn query_by_vector(
        &self,
        normalized_query: &[f32],
        n_results: usize,
    ) -> anyhow::Result<Vec<(f32, usize)>> {
        let results = self
            .hnsw
            .search(normalized_query, n_results, 1024, &self.storage, None)?;
        Ok(results.into_iter().map(|(id, dist)| (dist, id)).collect())
    }
}

fn pairwise_room_similarity(texts_a: &[&str], texts_b: &[&str]) -> f64 {
    if texts_a.is_empty() || texts_b.is_empty() {
        return 0.0;
    }
    let mut total_sim = 0.0_f64;
    let mut count = 0_usize;
    for text_a in texts_a {
        let words_a: std::collections::HashSet<_> = text_a.split_whitespace().collect();
        if words_a.is_empty() {
            continue;
        }
        let mut best_sim = 0.0_f64;
        for text_b in texts_b {
            let words_b: std::collections::HashSet<_> = text_b.split_whitespace().collect();
            if words_b.is_empty() {
                continue;
            }
            let intersection = words_a.intersection(&words_b).count() as f64;
            let union = words_a.union(&words_b).count() as f64;
            let sim = if union > 0.0 {
                intersection / union
            } else {
                0.0
            };
            if sim > best_sim {
                best_sim = sim;
            }
        }
        total_sim += best_sim;
        count += 1;
    }
    if count > 0 {
        total_sim / count as f64
    } else {
        0.0
    }
}

fn normalize_embedding(embedding: &[f32]) -> Vec<f32> {
    let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        return embedding.to_vec();
    }
    embedding.iter().map(|x| x / norm).collect()
}

fn naive_similarity(query: &str, content: &str) -> f64 {
    let query_words: std::collections::HashSet<_> = query.split_whitespace().collect();
    let content_words: std::collections::HashSet<_> = content.split_whitespace().collect();

    if query_words.is_empty() || content_words.is_empty() {
        return 0.0;
    }

    let intersection = query_words.intersection(&content_words).count();
    let union = query_words.union(&content_words).count();

    intersection as f64 / union as f64
}

/// Environment variable that disables the manifest validation in
/// [`PalaceDb::open_with_embedder`] (mp-016 / ADR-8).
///
/// Intended exclusively for tests and the planned `mpr migrate
/// --re-embed` flow, where we deliberately want to open a palace with
/// a different embedder. Setting it always emits a `warn!` so it
/// shows up in logs.
pub const SKIP_MANIFEST_CHECK_ENV: &str = "MEMPALACE_SKIP_MANIFEST_CHECK";

/// Read-or-write the embedding manifest as part of an embedder-aware
/// `PalaceDb::open` call. See [`PalaceDb::open_with_embedder`] for the
/// full contract.
fn validate_or_write_manifest(
    palace_path: &std::path::Path,
    embedder: &dyn crate::embed::Embedder,
    model_name: &str,
    drawer_count: usize,
) -> anyhow::Result<()> {
    use crate::embed::EmbeddingManifest;

    // Check the override first so we can warn and bail before touching
    // the disk.
    if std::env::var(SKIP_MANIFEST_CHECK_ENV)
        .map(|v| !v.is_empty() && v != "0")
        .unwrap_or(false)
    {
        tracing::warn!(
            target: "mempalace::manifest",
            env = SKIP_MANIFEST_CHECK_ENV,
            "embedding manifest validation skipped via env override (test/migration only)"
        );
        return Ok(());
    }

    match EmbeddingManifest::read(palace_path)? {
        Some(manifest) => {
            // mp-016: present → validate. Any mismatch is fatal and
            // surfaces as `MempalaceError::ManifestMismatch` once it
            // bubbles through the `?`-driven anyhow chain (the wrapper
            // `From<ManifestMismatch> for MempalaceError` is what makes
            // the typed error available to library callers; CLI users
            // see the same actionable message either way).
            manifest
                .validate_against(embedder)
                .map_err(crate::error::MempalaceError::ManifestMismatch)?;
            Ok(())
        }
        None => {
            // mp-015 / mp-016: absent. Two cases:
            //   1. Fresh palace (no drawers yet) → silently write the
            //      manifest so the next open is validated.
            //   2. Legacy palace (drawers exist, no manifest) → warn
            //      and write best-effort. We can't retrofit-validate
            //      the existing vectors but at least we lock in the
            //      identity going forward.
            if drawer_count > 0 {
                tracing::warn!(
                    target: "mempalace::manifest",
                    palace = %palace_path.display(),
                    drawers = drawer_count,
                    fingerprint = %embedder.fingerprint(),
                    "legacy palace has drawers but no embedding.json; writing best-effort manifest from active embedder. If this is the wrong embedder, run `mpr migrate --re-embed`."
                );
            }
            let manifest = EmbeddingManifest::from_embedder(embedder, model_name);
            EmbeddingManifest::write(palace_path, &manifest)?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_naive_similarity() {
        let sim = naive_similarity("hello world", "hello world");
        assert!((sim - 1.0).abs() < 1e-6);

        let sim = naive_similarity("hello world", "hello");
        assert!(sim > 0.3 && sim < 0.7);

        let sim = naive_similarity("hello world", "completely different");
        assert!(sim < 0.1);
    }

    /// #1498 regression: a non-existent path classifies as `Missing`.
    #[test]
    fn test_classify_palace_missing_dir() {
        let temp = tempfile::tempdir().unwrap();
        let palace = temp.path().join("nope");
        assert_eq!(classify_palace(&palace), PalaceState::Missing);
    }

    /// #1498 regression: directory exists but no collection JSON — the user
    /// has run `init` but not `mine`. Hint must be `mpr mine`, not `mpr init`.
    #[test]
    fn test_classify_palace_not_initialized_when_dir_only() {
        let temp = tempfile::tempdir().unwrap();
        let palace = temp.path().join("palace");
        std::fs::create_dir_all(&palace).unwrap();
        assert_eq!(classify_palace(&palace), PalaceState::NotInitialized);
    }

    /// #1498 regression: collection JSON exists but parses to zero documents.
    #[test]
    fn test_classify_palace_empty_when_no_documents() {
        let temp = tempfile::tempdir().unwrap();
        let palace = temp.path().join("palace");
        std::fs::create_dir_all(&palace).unwrap();
        let mut db = PalaceDb::open(&palace).unwrap();
        db.flush().unwrap();
        assert_eq!(classify_palace(&palace), PalaceState::Empty);
    }

    /// #1498 regression: a palace with at least one drawer classifies as
    /// `Ready` so the caller skips the actionable hint and prints stats.
    #[test]
    fn test_classify_palace_ready_when_documents_exist() {
        let temp = tempfile::tempdir().unwrap();
        let palace = temp.path().join("palace");
        std::fs::create_dir_all(&palace).unwrap();
        let mut db = PalaceDb::open(&palace).unwrap();
        db.add(
            &[("d1", "verbatim chunk")],
            &[&[("wing", "people"), ("room", "today")]],
        )
        .unwrap();
        db.flush().unwrap();
        assert_eq!(classify_palace(&palace), PalaceState::Ready);
    }

    /// mp-031 regression: secrets in drawer bodies must be redacted **before**
    /// they're persisted. We round-trip a chunk that contains a fake OpenAI
    /// key through `add()` + `flush()` + a fresh `open()` and assert the raw
    /// key is not present on disk.
    #[test]
    fn test_add_redacts_openai_key_before_storage() {
        let temp = tempfile::tempdir().unwrap();
        let palace = temp.path().join("palace");
        std::fs::create_dir_all(&palace).unwrap();

        let raw_key = "sk-abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ1234";
        let body = format!("the leaked key was {} please rotate it", raw_key);

        {
            let mut db = PalaceDb::open(&palace).unwrap();
            db.add(
                &[("d-secret", body.as_str())],
                &[&[("wing", "ops"), ("room", "incident")]],
            )
            .unwrap();
            db.flush().unwrap();
        }

        // Re-open from disk and inspect the persisted document.
        let db = PalaceDb::open(&palace).unwrap();
        let stored = db._get_document("d-secret").expect("drawer present");
        assert!(
            stored.content.contains("<REDACTED:OPENAI_KEY>"),
            "redaction placeholder missing: {}",
            stored.content
        );
        assert!(
            !stored.content.contains(raw_key),
            "raw key leaked into storage: {}",
            stored.content
        );
        // Surrounding prose is preserved.
        assert!(stored.content.contains("the leaked key was"));
        assert!(stored.content.contains("please rotate it"));
    }

    /// mp-031: `upsert_documents` is the other canonical add path
    /// (diary_ingest, sweeper, compress). It must redact too.
    #[test]
    fn test_upsert_documents_redacts_before_storage() {
        let temp = tempfile::tempdir().unwrap();
        let palace = temp.path().join("palace");
        std::fs::create_dir_all(&palace).unwrap();

        let raw_token = "ghp_abcdefghijklmnopqrstuvwxyz0123456789AB";
        let body = format!("token={}", raw_token);

        let mut db = PalaceDb::open(&palace).unwrap();
        let mut meta: HashMap<String, serde_json::Value> = HashMap::new();
        meta.insert("wing".into(), serde_json::json!("ops"));
        meta.insert("room".into(), serde_json::json!("creds"));
        db.upsert_documents(&[("d-gh".to_string(), body, meta)])
            .unwrap();
        db.flush().unwrap();

        let db = PalaceDb::open(&palace).unwrap();
        let stored = db._get_document("d-gh").expect("drawer present");
        assert!(stored.content.contains("<REDACTED:GITHUB_TOKEN>"));
        assert!(!stored.content.contains(raw_token));
    }

    /// mp-032 wiring: `PalaceDb::add_drawer_with_dedup` honours the
    /// rolling-window dedup. First call inserts; second call with the
    /// same content (within the window) skips and returns `None`. We
    /// inject a fresh `WindowedDedup` so this test does not entangle
    /// with the process-global window other tests share.
    #[test]
    fn test_add_drawer_with_dedup_skips_duplicate_within_window() {
        let temp = tempfile::tempdir().unwrap();
        let palace = temp.path().join("palace");
        std::fs::create_dir_all(&palace).unwrap();

        let mut db = PalaceDb::open(&palace).unwrap();
        let dedup =
            crate::dedup_window::WindowedDedup::new(std::time::Duration::from_secs(300), 64);
        let meta = [("wing", "ops"), ("room", "shipping")];

        let first = db
            .add_drawer_with_dedup(&dedup, "d-1", "release notes v0.1", &meta)
            .expect("first add_drawer call should succeed");
        assert_eq!(first, Some("d-1".to_string()));
        assert_eq!(db.count(), 1);

        let second = db
            .add_drawer_with_dedup(&dedup, "d-1-clone", "release notes v0.1", &meta)
            .expect("second add_drawer call should succeed");
        assert_eq!(second, None, "duplicate content should be skipped");
        assert_eq!(db.count(), 1, "no new drawer must be inserted on duplicate");
    }

    /// mp-032 wiring: whitespace differences are folded by the dedup
    /// hash so `"foo"` and `"  foo  "` collapse into one drawer.
    #[test]
    fn test_add_drawer_with_dedup_normalises_whitespace() {
        let temp = tempfile::tempdir().unwrap();
        let palace = temp.path().join("palace");
        std::fs::create_dir_all(&palace).unwrap();

        let mut db = PalaceDb::open(&palace).unwrap();
        let dedup =
            crate::dedup_window::WindowedDedup::new(std::time::Duration::from_secs(300), 64);
        let meta = [("wing", "ops"), ("room", "shipping")];

        assert_eq!(
            db.add_drawer_with_dedup(&dedup, "d-a", "foo", &meta)
                .unwrap(),
            Some("d-a".to_string())
        );
        assert_eq!(
            db.add_drawer_with_dedup(&dedup, "d-b", "  foo  ", &meta)
                .unwrap(),
            None,
            "trimmed whitespace should still hit the dedup window"
        );
        assert_eq!(db.count(), 1);
    }

    // -----------------------------------------------------------------
    // mp-016: `PalaceDb::open_with_embedder` manifest validation.
    // -----------------------------------------------------------------

    /// mp-016: opening a fresh palace with an embedder writes the
    /// manifest as a side-effect.
    #[cfg(not(windows))]
    #[test]
    fn test_open_with_embedder_writes_manifest_on_fresh_palace() {
        let temp = tempfile::tempdir().unwrap();
        let palace = temp.path().join("palace");
        std::fs::create_dir_all(&palace).unwrap();

        let embedder = crate::embed::NullEmbedder::new(384);
        let _db = PalaceDb::open_with_embedder(&palace, Arc::new(embedder), "null-test").unwrap();

        let manifest = crate::embed::EmbeddingManifest::read(&palace)
            .unwrap()
            .expect("manifest must be written on first open");
        assert_eq!(manifest.dim, 384);
        assert_eq!(manifest.fingerprint, "null:384");
        assert_eq!(manifest.model_name, "null-test");
    }

    /// mp-016: re-opening with the same embedder is a no-op (validation
    /// passes; the manifest is not rewritten with a fresh `created_at`).
    #[test]
    fn test_open_with_embedder_validates_existing_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let palace = temp.path().join("palace");
        std::fs::create_dir_all(&palace).unwrap();

        let embedder = crate::embed::NullEmbedder::new(384);
        let _ =
            PalaceDb::open_with_embedder(&palace, Arc::new(embedder.clone()), "null-test").unwrap();
        let original = crate::embed::EmbeddingManifest::read(&palace)
            .unwrap()
            .unwrap();

        // Re-open and confirm the manifest is unchanged byte-for-byte.
        let _ = PalaceDb::open_with_embedder(&palace, Arc::new(embedder), "null-test").unwrap();
        let after = crate::embed::EmbeddingManifest::read(&palace)
            .unwrap()
            .unwrap();
        assert_eq!(original, after);
    }

    /// mp-016: a dimension mismatch returns the actionable error.
    #[cfg(not(windows))]
    #[test]
    fn test_open_with_embedder_rejects_dim_change() {
        let temp = tempfile::tempdir().unwrap();
        let palace = temp.path().join("palace");
        std::fs::create_dir_all(&palace).unwrap();

        // Record at 384.
        let recorded = crate::embed::NullEmbedder::new(384);
        let _ = PalaceDb::open_with_embedder(&palace, Arc::new(recorded), "null-test").unwrap();

        // Open at 768 — must fail loud.
        let runtime = crate::embed::NullEmbedder::new(768);
        let err = match PalaceDb::open_with_embedder(&palace, Arc::new(runtime), "null-test") {
            Ok(_) => panic!("dim mismatch must be rejected"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("mpr migrate --re-embed"),
            "error must point at recovery command: {msg}"
        );
        assert!(msg.contains("384"), "error must show recorded dim: {msg}");
        assert!(msg.contains("768"), "error must show runtime dim: {msg}");

        // Confirm the typed error is reachable through the chain.
        let chain = err.chain();
        let mut found = false;
        for source in chain {
            if source
                .downcast_ref::<crate::embed::ManifestMismatch>()
                .is_some()
            {
                found = true;
                break;
            }
        }
        assert!(found, "ManifestMismatch must be in the error chain");
    }

    /// mp-016: the `MEMPALACE_SKIP_MANIFEST_CHECK=1` override lets a
    /// mismatched embedder open the palace anyway, for tests and the
    /// future re-embed migration. Other tests in the suite serialise on
    /// the env-mutex so we don't race with them.
    #[test]
    fn test_open_with_embedder_env_override_skips_validation() {
        let _guard = crate::test_env_lock().lock().unwrap();
        // SAFETY: serialised via test_env_lock; no concurrent env access.
        unsafe { std::env::remove_var(SKIP_MANIFEST_CHECK_ENV) };

        let temp = tempfile::tempdir().unwrap();
        let palace = temp.path().join("palace");
        std::fs::create_dir_all(&palace).unwrap();

        // Record at 384.
        let recorded = crate::embed::NullEmbedder::new(384);
        let _ = PalaceDb::open_with_embedder(&palace, Arc::new(recorded), "null-test").unwrap();

        // Open at 768 with the override set — must succeed.
        let runtime = crate::embed::NullEmbedder::new(768);
        // SAFETY: serialised via test_env_lock; no concurrent env access.
        unsafe { std::env::set_var(SKIP_MANIFEST_CHECK_ENV, "1") };
        let res = PalaceDb::open_with_embedder(&palace, Arc::new(runtime), "null-test");
        // SAFETY: serialised via test_env_lock; no concurrent env access.
        unsafe { std::env::remove_var(SKIP_MANIFEST_CHECK_ENV) };
        assert!(
            res.is_ok(),
            "override must allow mismatched open: {:?}",
            res.err()
        );
    }

    /// mp-016: legacy palace (existing drawers, no `embedding.json`)
    /// gets a best-effort manifest written on next open. The drawers
    /// remain untouched.
    #[test]
    fn test_open_with_embedder_writes_legacy_manifest_with_drawers() {
        let temp = tempfile::tempdir().unwrap();
        let palace = temp.path().join("palace");
        std::fs::create_dir_all(&palace).unwrap();

        // Simulate a legacy palace: drawers exist, no manifest.
        {
            let mut db = PalaceDb::open(&palace).unwrap();
            db.add(
                &[("d-legacy", "an old drawer from before mp-015")],
                &[&[("wing", "ops"), ("room", "history")]],
            )
            .unwrap();
            db.flush().unwrap();
        }
        assert!(
            !crate::embed::EmbeddingManifest::path(&palace).is_file(),
            "legacy palace must start with no manifest"
        );

        // Now open with an embedder — manifest should be best-effort
        // written from the active embedder.
        let embedder = crate::embed::NullEmbedder::new(384);
        let db = PalaceDb::open_with_embedder(&palace, Arc::new(embedder), "null-test").unwrap();
        assert_eq!(db.count(), 1, "drawer count must be preserved");

        let manifest = crate::embed::EmbeddingManifest::read(&palace)
            .unwrap()
            .unwrap();
        assert_eq!(manifest.dim, 384);
        assert_eq!(manifest.fingerprint, "null:384");
    }
}
