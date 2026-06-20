use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

// SAFETY: `KnowledgeGraph` is `Send + Sync` because `conn` is wrapped in
// a `Mutex` which makes interior mutation safe across threads. The Mutex
// serialises all access, and SQLite's WAL mode handles concurrent reads at
// the C level when the Mutex is not held.
#[non_exhaustive]
pub struct KnowledgeGraph {
    conn: Mutex<Connection>,
}

// KnowledgeGraph is Send + Sync because Mutex<Connection> is Send + Sync.
// No unsafe impl needed — the compiler derives these automatically.

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Triple {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub confidence: Option<f64>,
    pub source_closet: Option<String>,
    pub source_file: Option<String>,
    // RFC 002 §5.5 provenance (#1314): adapter-supplied drawer pointer and
    // adapter identifier. Both default to None for callers that don't carry
    // adapter context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_drawer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_name: Option<String>,
    pub current: bool,
    // Transaction time: when this fact was first recorded (t_created) and
    // when it was superseded (t_expired, NULL = still current).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t_created: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t_expired: Option<String>,
    // mp-027 (#27): typed memory edges. `edge_kind` is the variant name
    // (e.g. "has_tag") and `weight` is the per-edge traversal weight. Both
    // default to `None` for triples that were not created via the typed-edge
    // API and are read from the typed columns. Callers that want the typed
    // view should use [`MemoryEdgeKind::from_kind_and_weight`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Entity {
    pub id: String,
    pub name: String,
    pub entity_type: String,
    pub properties: serde_json::Value,
}

#[derive(Debug)]
#[non_exhaustive]
pub struct KgStats {
    pub total_entities: usize,
    pub total_triples: usize,
    pub current_facts: usize,
    pub expired_facts: usize,
    pub relationship_types: Vec<String>,
}

/// A point-in-time snapshot of the knowledge graph. Contains aggregate counts
/// and top-degree entities for fast synchronous lookups without scanning the
/// full node set every time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct GraphSnapshot {
    pub snapshot_id: String,
    pub total_nodes: usize,
    pub total_edges: usize,
    pub top_degrees: std::collections::HashMap<String, usize>,
    pub created_at: String,
    pub reset_at: Option<String>,
}

/// Enriched query result envelope returned by snapshot-aware KG queries.
/// Wraps the fact list with metadata about the data source.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct GraphQueryResult {
    pub facts: Vec<EntityQueryResult>,
    pub count: usize,
    pub total_nodes: Option<usize>,
    pub total_edges: Option<usize>,
    pub from_snapshot: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ClusterEntry {
    pub id: String,
    pub name: Option<String>,
    pub centroid: Vec<f32>,
    pub member_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct EntityQueryResult {
    pub direction: String,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub confidence: Option<f64>,
    pub source_closet: Option<String>,
    pub source_file: Option<String>,
    // RFC 002 §5.5 provenance (#1314): see Triple::source_drawer_id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_drawer_id: Option<String>,
    pub current: bool,
    // Transaction time: when this fact was first recorded (t_created) and
    // when it was superseded (t_expired, NULL = still current).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t_created: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t_expired: Option<String>,
    // mp-027 (#27): typed memory edges. See [`Triple::edge_kind`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
}

impl KnowledgeGraph {
    pub fn open(db_path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;
        // Enable WAL mode for better concurrent read performance and reduced SQLITE_BUSY risk
        let _: String = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
        let kg = Self {
            conn: Mutex::new(conn),
        };
        kg.init_db()?;
        Ok(kg)
    }

    fn init_db(&self) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute_batch(
            "
            CREATE TABLE IF NOT EXISTS entities (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                entity_type TEXT DEFAULT 'unknown',
                properties TEXT DEFAULT '{}',
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS triples (
                id TEXT PRIMARY KEY,
                subject TEXT NOT NULL,
                predicate TEXT NOT NULL,
                object TEXT NOT NULL,
                valid_from TEXT,
                valid_to TEXT,
                confidence REAL DEFAULT 1.0,
                source_closet TEXT,
                source_file TEXT,
                source_drawer_id TEXT,
                adapter_name TEXT,
                extracted_at TEXT DEFAULT CURRENT_TIMESTAMP,
                t_created TEXT NOT NULL DEFAULT (datetime('now')),
                t_expired TEXT,
                -- mp-027 (issue #27): typed memory edges (HasTag, RelatesTo,
                -- Supersedes, ...) and their traversal weights. `edge_kind` is
                -- the variant name (e.g. has_tag); `weight` is the per-edge
                -- traversal weight, separate from `confidence`.
                edge_kind TEXT,
                weight REAL,
                FOREIGN KEY (subject) REFERENCES entities(id),
                FOREIGN KEY (object) REFERENCES entities(id)
            );

CREATE INDEX IF NOT EXISTS idx_triples_subject ON triples(subject);
            CREATE INDEX IF NOT EXISTS idx_triples_object ON triples(object);
            CREATE INDEX IF NOT EXISTS idx_triples_predicate ON triples(predicate);
            CREATE INDEX IF NOT EXISTS idx_triples_valid ON triples(valid_from, valid_to);
            -- idx_triples_edge_kind is created lazily in migrate_schema() so
            -- palaces created before edge_kind was added don't fail.

            CREATE TABLE IF NOT EXISTS episodes (
                id TEXT PRIMARY KEY,
                drawer_id TEXT NOT NULL,
                query TEXT NOT NULL,
                outcome TEXT NOT NULL,
                feedback_at TEXT DEFAULT CURRENT_TIMESTAMP
            );

            CREATE TABLE IF NOT EXISTS clusters (
                id TEXT PRIMARY KEY,
                name TEXT,
                centroid BLOB,
                member_count INTEGER,
                created_at TEXT,
                updated_at TEXT
            );
            ",
        )?;
        self.migrate_schema()?;
        Ok(())
    }

    /// Backwards-compatible schema migration for older `triples` tables
    /// (#1314 RFC 002 §5.5, #27 mp-027). Fresh palaces already have
    /// `source_drawer_id`, `adapter_name`, `t_created`, `t_expired`,
    /// `edge_kind`, and `weight` from the `CREATE TABLE` above, so this is
    /// a no-op. Palaces created before those columns were added must be
    /// migrated in place — SQLite has no `ADD COLUMN IF NOT EXISTS`, so we
    /// introspect the schema and only issue the ALTER when the column is
    /// missing.
    fn migrate_schema(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("PRAGMA table_info(triples)")?;
        let names: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if !names.iter().any(|n| n == "source_drawer_id") {
            conn.execute("ALTER TABLE triples ADD COLUMN source_drawer_id TEXT", [])?;
        }
        if !names.iter().any(|n| n == "adapter_name") {
            conn.execute("ALTER TABLE triples ADD COLUMN adapter_name TEXT", [])?;
        }
        if !names.iter().any(|n| n == "t_created") {
            conn.execute("ALTER TABLE triples ADD COLUMN t_created TEXT", [])?;
        }
        if !names.iter().any(|n| n == "t_expired") {
            conn.execute("ALTER TABLE triples ADD COLUMN t_expired TEXT", [])?;
        }
        // mp-027 (#27): typed memory edges with traversal weights. `edge_kind`
        // is the variant name (e.g. "has_tag"); `weight` is the per-edge
        // traversal weight, separate from `confidence`.
        if !names.iter().any(|n| n == "edge_kind") {
            conn.execute("ALTER TABLE triples ADD COLUMN edge_kind TEXT", [])?;
        }
        if !names.iter().any(|n| n == "weight") {
            conn.execute("ALTER TABLE triples ADD COLUMN weight REAL", [])?;
        }
        // The typed-edge index is created after the columns are guaranteed
        // to exist, so palaces that pre-date edge_kind don't fail at the
        // CREATE INDEX statement. SQLite has no `CREATE INDEX IF NOT EXISTS`
        // safety here because the column not existing is what fails.
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_triples_edge_kind ON triples(edge_kind)",
            [],
        )?;
        // Backfill existing rows that lack t_created
        conn.execute(
            "UPDATE triples SET t_created = COALESCE(valid_from, extracted_at) WHERE t_created IS NULL",
            [],
        )?;
        conn.execute(
            "UPDATE triples SET t_expired = NULL WHERE t_expired IS NULL",
            [],
        )?;
        // Backfill edge_kind/weight from legacy encodings. There were two
        // forms in the wild:
        //   1. plain predicate == canonical kind name (e.g. "has_tag")
        //   2. "<kind>_<weight>" pattern from `relations.rs::create_relation`
        //      (e.g. "relates_to_0.80")
        // Both forms are mapped to the typed columns; unknown predicates are
        // left alone.
        for kind in &[
            "has_tag",
            "in_cluster",
            "supersedes",
            "contradicts",
            "derived_from",
        ] {
            conn.execute(
                "UPDATE triples SET edge_kind = ?1 WHERE edge_kind IS NULL AND predicate = ?2",
                rusqlite::params![kind, kind],
            )?;
        }
        // Backfill the canonical weight for the fixed-weight kinds that
        // already had a typed edge_kind (form 1).
        conn.execute(
            "UPDATE triples SET weight = 0.8 WHERE edge_kind = 'has_tag' AND weight IS NULL",
            [],
        )?;
        conn.execute(
            "UPDATE triples SET weight = 0.6 WHERE edge_kind = 'in_cluster' AND weight IS NULL",
            [],
        )?;
        conn.execute(
            "UPDATE triples SET weight = 0.9 WHERE edge_kind = 'supersedes' AND weight IS NULL",
            [],
        )?;
        conn.execute(
            "UPDATE triples SET weight = 0.3 WHERE edge_kind = 'contradicts' AND weight IS NULL",
            [],
        )?;
        conn.execute(
            "UPDATE triples SET weight = 0.7 WHERE edge_kind = 'derived_from' AND weight IS NULL",
            [],
        )?;
        // Legacy "<kind>_<weight>" pattern (form 2). Only the six documented
        // kinds participate; predicates with an unknown prefix are left
        // untouched.
        conn.execute(
            "UPDATE triples
             SET edge_kind = 'has_tag',
                 weight = 0.8
             WHERE edge_kind IS NULL
               AND predicate LIKE 'has_tag\\_%' ESCAPE '\\'",
            [],
        )?;
        conn.execute(
            "UPDATE triples
             SET edge_kind = 'in_cluster',
                 weight = 0.6
             WHERE edge_kind IS NULL
               AND predicate LIKE 'in_cluster\\_%' ESCAPE '\\'",
            [],
        )?;
        conn.execute(
            "UPDATE triples
             SET edge_kind = 'supersedes',
                 weight = 0.9
             WHERE edge_kind IS NULL
               AND predicate LIKE 'supersedes\\_%' ESCAPE '\\'",
            [],
        )?;
        conn.execute(
            "UPDATE triples
             SET edge_kind = 'contradicts',
                 weight = 0.3
             WHERE edge_kind IS NULL
               AND predicate LIKE 'contradicts\\_%' ESCAPE '\\'",
            [],
        )?;
        conn.execute(
            "UPDATE triples
             SET edge_kind = 'derived_from',
                 weight = 0.7
             WHERE edge_kind IS NULL
               AND predicate LIKE 'derived_from\\_%' ESCAPE '\\'",
            [],
        )?;
        // `relates_to_<weight>` carries its own weight; peel the suffix.
        conn.execute(
            "UPDATE triples
             SET edge_kind = 'relates_to',
                 weight = CAST(substr(predicate, length('relates_to_') + 1) AS REAL)
             WHERE edge_kind IS NULL
               AND predicate LIKE 'relates_to\\_%' ESCAPE '\\'",
            [],
        )?;
        Ok(())
    }

    fn entity_id(name: &str) -> String {
        name.to_lowercase().replace(' ', "_").replace('\'', "")
    }

    pub fn add_entity(
        &self,
        name: &str,
        entity_type: &str,
        properties: Option<&serde_json::Value>,
    ) -> anyhow::Result<String> {
        let eid = Self::entity_id(name);
        let props = match properties {
            Some(p) => serde_json::to_string(p)?,
            None => "{}".to_string(),
        };
        self.conn.lock().unwrap().execute(
            "INSERT OR REPLACE INTO entities (id, name, entity_type, properties) VALUES (?1, ?2, ?3, ?4)",
            params![eid, name, entity_type, props],
        )?;
        Ok(eid)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_triple(
        &self,
        subject: &str,
        predicate: &str,
        object: &str,
        valid_from: Option<&str>,
        valid_to: Option<&str>,
        confidence: Option<f64>,
        source_closet: Option<&str>,
        source_file: Option<&str>,
        // RFC 002 §5.5 provenance (#1314): adapter-supplied drawer pointer
        // and adapter identifier. Both default to `None` so existing callers
        // stay source-compatible.
        source_drawer_id: Option<&str>,
        adapter_name: Option<&str>,
    ) -> anyhow::Result<String> {
        // Canonicalize temporal values at the KG boundary (#1214 / mr-gvpc).
        // `sanitize_iso_temporal` rejects naive datetimes, non-UTC offsets, and
        // partial dates — and normalizes `+00:00` → `Z`. Callers that bypass
        // the MCP layer (e.g. internal bulk imports) still get the same shape
        // guarantees so KG TEXT comparisons stay correct.
        let valid_from: Option<String> =
            crate::config::sanitize_iso_temporal(valid_from, "valid_from")?;
        let valid_to: Option<String> = crate::config::sanitize_iso_temporal(valid_to, "valid_to")?;

        // Reject inverted intervals (#1214): a triple with valid_to < valid_from
        // would never satisfy `valid_from <= as_of AND valid_to >= as_of`, so it
        // would be invisible to every query — silently corrupt. Open intervals
        // and point-in-time facts (valid_from == valid_to) remain accepted.
        if let (Some(vf), Some(vt)) = (valid_from.as_deref(), valid_to.as_deref()) {
            if vt < vf {
                anyhow::bail!(
                    "valid_to={vt:?} is before valid_from={vf:?}; an inverted interval would be invisible to every KG query"
                );
            }
        }
        let sub_id = Self::entity_id(subject);
        let obj_id = Self::entity_id(object);
        let pred = predicate.to_lowercase().replace(' ', "_");

        self.conn.lock().unwrap().execute(
            "INSERT OR IGNORE INTO entities (id, name) VALUES (?1, ?2)",
            params![sub_id, subject],
        )?;
        self.conn.lock().unwrap().execute(
            "INSERT OR IGNORE INTO entities (id, name) VALUES (?1, ?2)",
            params![obj_id, object],
        )?;

        let check_exists: Result<String, _> = self.conn.lock().unwrap().query_row(
            "SELECT id FROM triples WHERE subject=?1 AND predicate=?2 AND object=?3 AND valid_to IS NULL",
            params![sub_id, pred, obj_id],
            |row| row.get(0),
        );

        if let Ok(existing_id) = check_exists {
            return Ok(existing_id);
        }

        // Auto-resolve conflicts: if same subject+predicate has different object,
        // invalidate the old triple first
        let conflicting: Result<String, _> = self.conn.lock().unwrap().query_row(
            "SELECT id FROM triples WHERE subject=?1 AND predicate=?2 AND valid_to IS NULL AND object<>?3",
            params![sub_id, pred, obj_id],
            |row| row.get(0),
        );

        if let Ok(conflict_id) = conflicting {
            // Invalidate the conflicting triple at the start of the new fact when known,
            // otherwise fall back to the current timestamp.
            let conflict_end = valid_from
                .clone()
                .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
            self.conn.lock().unwrap().execute(
                "UPDATE triples SET valid_to=?1 WHERE id=?2",
                params![conflict_end, conflict_id],
            )?;
            // Log conflict resolution - triple_id not yet assigned, use format
            tracing::info!(
                "Auto-resolved conflict: invalidated {} for subject={} predicate={}",
                conflict_id,
                subject,
                predicate
            );
        }

        let now = chrono::Utc::now().to_rfc3339();
        let triple_id = format!("t_{}_{}_{}_{}", sub_id, pred, obj_id, &now[..8]);

        self.conn.lock().unwrap().execute(
            "INSERT INTO triples (id, subject, predicate, object, valid_from, valid_to, confidence, source_closet, source_file, source_drawer_id, adapter_name, t_created, t_expired, edge_kind, weight)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                triple_id,
                sub_id,
                pred,
                obj_id,
                valid_from,
                valid_to,
                confidence.unwrap_or(1.0),
                source_closet,
                source_file,
                source_drawer_id,
                adapter_name,
                now,
                Option::<String>::None,
                Option::<String>::None,
                Option::<f64>::None
            ],
        )?;

        Ok(triple_id)
    }

    pub fn invalidate(
        &self,
        subject: &str,
        predicate: &str,
        object: &str,
        ended: Option<&str>,
    ) -> anyhow::Result<()> {
        let sub_id = Self::entity_id(subject);
        let obj_id = Self::entity_id(object);
        let pred = predicate.to_lowercase().replace(' ', "_");
        let ended_date = ended
            .map(|s| s.to_string())
            .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());

        self.conn.lock().unwrap().execute(
            "UPDATE triples SET valid_to=?1 WHERE subject=?2 AND predicate=?3 AND object=?4 AND valid_to IS NULL",
            params![ended_date, sub_id, pred, obj_id],
        )?;

        Ok(())
    }

    pub fn query_entity(
        &self,
        name: &str,
        as_of: Option<&str>,
        tt_as_of: Option<&str>,
        direction: &str,
    ) -> anyhow::Result<Vec<EntityQueryResult>> {
        let eid = Self::entity_id(name);
        let mut results = Vec::new();

        if direction == "outgoing" || direction == "both" {
            results.extend(self.query_outgoing(&eid, as_of, tt_as_of)?);
        }

        if direction == "incoming" || direction == "both" {
            results.extend(self.query_incoming(&eid, as_of, tt_as_of)?);
        }

        Ok(results)
    }

    fn query_outgoing(
        &self,
        eid: &str,
        as_of: Option<&str>,
        tt_as_of: Option<&str>,
    ) -> anyhow::Result<Vec<EntityQueryResult>> {
        let mut results = Vec::new();

        if let Some(date) = as_of {
            if let Some(tt) = tt_as_of {
                let _c = self.conn.lock().unwrap();
                let mut stmt = _c.prepare(
                    "SELECT t.*, e.name as obj_name FROM triples t JOIN entities e ON t.object = e.id WHERE t.subject = ?1 AND (t.valid_from IS NULL OR t.valid_from <= ?2) AND (t.valid_to IS NULL OR t.valid_to >= ?3) AND (t.t_created IS NULL OR t.t_created <= ?4) AND (t.t_expired IS NULL OR t.t_expired >= ?5)"
                )?;
                let mut rows = stmt.query(params![eid, date, date, tt, tt])?;
                while let Some(row) = rows.next()? {
                    results.push(self.row_to_entity_result(row, "outgoing", eid)?);
                }
            } else {
                let _c = self.conn.lock().unwrap();
                let mut stmt = _c.prepare(
                    "SELECT t.*, e.name as obj_name FROM triples t JOIN entities e ON t.object = e.id WHERE t.subject = ?1 AND (t.valid_from IS NULL OR t.valid_from <= ?2) AND (t.valid_to IS NULL OR t.valid_to >= ?3) AND (t.t_expired IS NULL OR t.t_expired > ?4)"
                )?;
                let mut rows = stmt.query(params![eid, date, date, date])?;
                while let Some(row) = rows.next()? {
                    results.push(self.row_to_entity_result(row, "outgoing", eid)?);
                }
            }
        } else {
            if let Some(tt) = tt_as_of {
                let _c = self.conn.lock().unwrap();
                let mut stmt = _c.prepare(
                    "SELECT t.*, e.name as obj_name FROM triples t JOIN entities e ON t.object = e.id WHERE t.subject = ?1 AND (t.t_created IS NULL OR t.t_created <= ?2) AND (t.t_expired IS NULL OR t.t_expired >= ?3)"
                )?;
                let mut rows = stmt.query(params![eid, tt, tt])?;
                while let Some(row) = rows.next()? {
                    results.push(self.row_to_entity_result(row, "outgoing", eid)?);
                }
            } else {
                let _c = self.conn.lock().unwrap();
                let mut stmt = _c.prepare(
                    "SELECT t.*, e.name as obj_name FROM triples t JOIN entities e ON t.object = e.id WHERE t.subject = ?1"
                )?;
                let rows = stmt.query_map(params![eid], |row| {
                    self.row_to_entity_result(row, "outgoing", eid)
                })?;
                for row in rows {
                    results.push(row?);
                }
            }
        }

        Ok(results)
    }

    fn query_incoming(
        &self,
        eid: &str,
        as_of: Option<&str>,
        tt_as_of: Option<&str>,
    ) -> anyhow::Result<Vec<EntityQueryResult>> {
        let mut results = Vec::new();

        if let Some(date) = as_of {
            if let Some(tt) = tt_as_of {
                let _c = self.conn.lock().unwrap();
                let mut stmt = _c.prepare(
                    "SELECT t.*, e.name as sub_name FROM triples t JOIN entities e ON t.subject = e.id WHERE t.object = ?1 AND (t.valid_from IS NULL OR t.valid_from <= ?2) AND (t.valid_to IS NULL OR t.valid_to >= ?3) AND (t.t_created IS NULL OR t.t_created <= ?4) AND (t.t_expired IS NULL OR t.t_expired >= ?5)"
                )?;
                let mut rows = stmt.query(params![eid, date, date, tt, tt])?;
                while let Some(row) = rows.next()? {
                    results.push(self.row_to_entity_result_incoming(row, "incoming", eid)?);
                }
            } else {
                let _c = self.conn.lock().unwrap();
                let mut stmt = _c.prepare(
                    "SELECT t.*, e.name as sub_name FROM triples t JOIN entities e ON t.subject = e.id WHERE t.object = ?1 AND (t.valid_from IS NULL OR t.valid_from <= ?2) AND (t.valid_to IS NULL OR t.valid_to >= ?3) AND (t.t_expired IS NULL OR t.t_expired > ?4)"
                )?;
                let mut rows = stmt.query(params![eid, date, date, date])?;
                while let Some(row) = rows.next()? {
                    results.push(self.row_to_entity_result_incoming(row, "incoming", eid)?);
                }
            }
        } else {
            if let Some(tt) = tt_as_of {
                let _c = self.conn.lock().unwrap();
                let mut stmt = _c.prepare(
                    "SELECT t.*, e.name as sub_name FROM triples t JOIN entities e ON t.subject = e.id WHERE t.object = ?1 AND (t.t_created IS NULL OR t.t_created <= ?2) AND (t.t_expired IS NULL OR t.t_expired >= ?3)"
                )?;
                let mut rows = stmt.query(params![eid, tt, tt])?;
                while let Some(row) = rows.next()? {
                    results.push(self.row_to_entity_result_incoming(row, "incoming", eid)?);
                }
            } else {
                let _c = self.conn.lock().unwrap();
                let mut stmt = _c.prepare(
                    "SELECT t.*, e.name as sub_name FROM triples t JOIN entities e ON t.subject = e.id WHERE t.object = ?1"
                )?;
                let rows = stmt.query_map(params![eid], |row| {
                    self.row_to_entity_result_incoming(row, "incoming", eid)
                })?;
                for row in rows {
                    results.push(row?);
                }
            }
        }

        Ok(results)
    }

    fn row_to_entity_result(
        &self,
        row: &rusqlite::Row,
        direction: &str,
        _subject: &str,
    ) -> rusqlite::Result<EntityQueryResult> {
        let valid_to: Option<String> = row.get("valid_to")?;
        Ok(EntityQueryResult {
            direction: direction.to_string(),
            subject: _subject.to_string(),
            predicate: row.get("predicate")?,
            object: row.get("obj_name")?,
            valid_from: row.get("valid_from")?,
            valid_to: valid_to.clone(),
            confidence: row.get("confidence")?,
            source_closet: row.get("source_closet")?,
            source_file: row.get("source_file")?,
            source_drawer_id: row.get("source_drawer_id")?,
            current: valid_to.is_none(),
            t_created: row.get("t_created")?,
            t_expired: row.get("t_expired")?,
            edge_kind: row.get("edge_kind")?,
            weight: row.get("weight")?,
        })
    }

    fn row_to_entity_result_incoming(
        &self,
        row: &rusqlite::Row,
        direction: &str,
        _object: &str,
    ) -> rusqlite::Result<EntityQueryResult> {
        let valid_to: Option<String> = row.get("valid_to")?;
        Ok(EntityQueryResult {
            direction: direction.to_string(),
            subject: row.get("sub_name")?,
            predicate: row.get("predicate")?,
            object: _object.to_string(),
            valid_from: row.get("valid_from")?,
            valid_to: valid_to.clone(),
            confidence: row.get("confidence")?,
            source_closet: row.get("source_closet")?,
            source_file: row.get("source_file")?,
            source_drawer_id: row.get("source_drawer_id")?,
            current: valid_to.is_none(),
            t_created: row.get("t_created")?,
            t_expired: row.get("t_expired")?,
            edge_kind: row.get("edge_kind")?,
            weight: row.get("weight")?,
        })
    }

    pub fn query_relationship(
        &self,
        predicate: &str,
        as_of: Option<&str>,
        tt_as_of: Option<&str>,
    ) -> anyhow::Result<Vec<Triple>> {
        let pred = predicate.to_lowercase().replace(' ', "_");
        let mut results = Vec::new();

        if let Some(date) = as_of {
            if let Some(tt) = tt_as_of {
                let _c = self.conn.lock().unwrap();
                let mut stmt = _c.prepare(
                    "SELECT t.*, s.name as sub_name, o.name as obj_name FROM triples t JOIN entities s ON t.subject = s.id JOIN entities o ON t.object = o.id WHERE t.predicate = ?1 AND (t.valid_from IS NULL OR t.valid_from <= ?2) AND (t.valid_to IS NULL OR t.valid_to >= ?3) AND (t.t_created IS NULL OR t.t_created <= ?4) AND (t.t_expired IS NULL OR t.t_expired >= ?5)"
                )?;
                let rows = stmt.query_map(params![pred, date, date, tt, tt], |row| {
                    self.row_to_triple(row, &pred)
                })?;
                for row in rows {
                    results.push(row?);
                }
            } else {
                let _c = self.conn.lock().unwrap();
                let mut stmt = _c.prepare(
                    "SELECT t.*, s.name as sub_name, o.name as obj_name FROM triples t JOIN entities s ON t.subject = s.id JOIN entities o ON t.object = o.id WHERE t.predicate = ?1 AND (t.valid_from IS NULL OR t.valid_from <= ?2) AND (t.valid_to IS NULL OR t.valid_to >= ?3)"
                )?;
                let rows = stmt.query_map(params![pred, date, date], |row| {
                    self.row_to_triple(row, &pred)
                })?;
                for row in rows {
                    results.push(row?);
                }
            }
        } else {
            if let Some(tt) = tt_as_of {
                let _c = self.conn.lock().unwrap();
                let mut stmt = _c.prepare(
                    "SELECT t.*, s.name as sub_name, o.name as obj_name FROM triples t JOIN entities s ON t.subject = s.id JOIN entities o ON t.object = o.id WHERE t.predicate = ?1 AND (t.t_created IS NULL OR t.t_created <= ?2) AND (t.t_expired IS NULL OR t.t_expired >= ?3)"
                )?;
                let rows =
                    stmt.query_map(params![pred, tt, tt], |row| self.row_to_triple(row, &pred))?;
                for row in rows {
                    results.push(row?);
                }
            } else {
                let _c = self.conn.lock().unwrap();
                let mut stmt = _c.prepare(
                    "SELECT t.*, s.name as sub_name, o.name as obj_name FROM triples t JOIN entities s ON t.subject = s.id JOIN entities o ON t.object = o.id WHERE t.predicate = ?1"
                )?;
                let rows = stmt.query_map(params![pred], |row| self.row_to_triple(row, &pred))?;
                for row in rows {
                    results.push(row?);
                }
            }
        }

        Ok(results)
    }

    fn row_to_triple(&self, row: &rusqlite::Row, predicate: &str) -> rusqlite::Result<Triple> {
        let valid_to: Option<String> = row.get("valid_to")?;
        Ok(Triple {
            subject: row.get("sub_name")?,
            predicate: predicate.to_string(),
            object: row.get("obj_name")?,
            valid_from: row.get("valid_from")?,
            valid_to: valid_to.clone(),
            confidence: row.get("confidence")?,
            source_closet: row.get("source_closet")?,
            source_file: row.get("source_file")?,
            source_drawer_id: row.get("source_drawer_id")?,
            adapter_name: row.get("adapter_name")?,
            current: valid_to.is_none(),
            t_created: row.get("t_created")?,
            t_expired: row.get("t_expired")?,
            edge_kind: row.get("edge_kind")?,
            weight: row.get("weight")?,
        })
    }

    pub fn timeline(&self, entity_name: Option<&str>) -> anyhow::Result<Vec<Triple>> {
        let mut results = Vec::new();

        if let Some(name) = entity_name {
            let eid = Self::entity_id(name);
            let _c = self.conn.lock().unwrap();
            let mut stmt = _c.prepare(
                "SELECT t.*, s.name as sub_name, o.name as obj_name FROM triples t JOIN entities s ON t.subject = s.id JOIN entities o ON t.object = o.id WHERE t.subject = ?1 OR t.object = ?1 ORDER BY t.valid_from ASC LIMIT 100"
            )?;
            let rows = stmt.query_map(params![eid], |row| {
                Ok(Triple {
                    subject: row.get("sub_name")?,
                    predicate: row.get("predicate")?,
                    object: row.get("obj_name")?,
                    valid_from: row.get("valid_from")?,
                    valid_to: row.get("valid_to")?,
                    confidence: row.get("confidence")?,
                    source_closet: row.get("source_closet")?,
                    source_file: row.get("source_file")?,
                    source_drawer_id: row.get("source_drawer_id")?,
                    adapter_name: row.get("adapter_name")?,
                    current: row.get::<_, Option<String>>("valid_to")?.is_none(),
                    t_created: row.get("t_created")?,
                    t_expired: row.get("t_expired")?,
                    edge_kind: row.get("edge_kind")?,
                    weight: row.get("weight")?,
                })
            })?;
            for row in rows {
                results.push(row?);
            }
        } else {
            let _c = self.conn.lock().unwrap();
            let mut stmt = _c.prepare(
                "SELECT t.*, s.name as sub_name, o.name as obj_name FROM triples t JOIN entities s ON t.subject = s.id JOIN entities o ON t.object = o.id ORDER BY t.valid_from ASC LIMIT 100"
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(Triple {
                    subject: row.get("sub_name")?,
                    predicate: row.get("predicate")?,
                    object: row.get("obj_name")?,
                    valid_from: row.get("valid_from")?,
                    valid_to: row.get("valid_to")?,
                    confidence: row.get("confidence")?,
                    source_closet: row.get("source_closet")?,
                    source_file: row.get("source_file")?,
                    source_drawer_id: row.get("source_drawer_id")?,
                    adapter_name: row.get("adapter_name")?,
                    current: row.get::<_, Option<String>>("valid_to")?.is_none(),
                    t_created: row.get("t_created")?,
                    t_expired: row.get("t_expired")?,
                    edge_kind: row.get("edge_kind")?,
                    weight: row.get("weight")?,
                })
            })?;
            for row in rows {
                results.push(row?);
            }
        }

        Ok(results)
    }

    /// Timeline filtered by transaction time using `tt_as_of`.
    /// When `tt_as_of` is `None`, falls back to current (same as `timeline`).
    pub fn timeline_for_transaction_time(
        &self,
        entity_name: Option<&str>,
        tt_as_of: Option<&str>,
    ) -> anyhow::Result<Vec<Triple>> {
        let mut results = Vec::new();

        if let Some(name) = entity_name {
            let eid = Self::entity_id(name);
            if let Some(tt) = tt_as_of {
                let _c = self.conn.lock().unwrap();
                let mut stmt = _c.prepare(
                    "SELECT t.*, s.name as sub_name, o.name as obj_name FROM triples t \
                     JOIN entities s ON t.subject = s.id \
                     JOIN entities o ON t.object = o.id \
                     WHERE (t.subject = ?1 OR t.object = ?1) \
                     AND (t.t_created IS NULL OR t.t_created <= ?2) \
                     AND (t.t_expired IS NULL OR t.t_expired >= ?3) \
                     ORDER BY t.valid_from ASC LIMIT 100",
                )?;
                let rows = stmt.query_map(params![eid, tt, tt], |row| {
                    Ok(Triple {
                        subject: row.get("sub_name")?,
                        predicate: row.get("predicate")?,
                        object: row.get("obj_name")?,
                        valid_from: row.get("valid_from")?,
                        valid_to: row.get("valid_to")?,
                        confidence: row.get("confidence")?,
                        source_closet: row.get("source_closet")?,
                        source_file: row.get("source_file")?,
                        source_drawer_id: row.get("source_drawer_id")?,
                        adapter_name: row.get("adapter_name")?,
                        current: row.get::<_, Option<String>>("valid_to")?.is_none(),
                        t_created: row.get("t_created")?,
                        t_expired: row.get("t_expired")?,
                        edge_kind: row.get("edge_kind")?,
                        weight: row.get("weight")?,
                    })
                })?;
                for row in rows {
                    results.push(row?);
                }
            } else {
                let now = chrono::Utc::now().to_rfc3339();
                let eid = Self::entity_id(name);
                let _c = self.conn.lock().unwrap();
                let mut stmt = _c.prepare(
                    "SELECT t.*, s.name as sub_name, o.name as obj_name FROM triples t \
                     JOIN entities s ON t.subject = s.id \
                     JOIN entities o ON t.object = o.id \
                     WHERE (t.subject = ?1 OR t.object = ?1) \
                     AND (t.t_created IS NULL OR t.t_created <= ?2) \
                     AND (t.t_expired IS NULL OR t.t_expired >= ?3) \
                     AND (t.valid_from IS NULL OR t.valid_from <= ?4) \
                     AND (t.valid_to IS NULL OR t.valid_to >= ?5) \
                     ORDER BY t.valid_from ASC LIMIT 100",
                )?;
                let rows = stmt.query_map(params![eid, now, now, now, now], |row| {
                    Ok(Triple {
                        subject: row.get("sub_name")?,
                        predicate: row.get("predicate")?,
                        object: row.get("obj_name")?,
                        valid_from: row.get("valid_from")?,
                        valid_to: row.get("valid_to")?,
                        confidence: row.get("confidence")?,
                        source_closet: row.get("source_closet")?,
                        source_file: row.get("source_file")?,
                        source_drawer_id: row.get("source_drawer_id")?,
                        adapter_name: row.get("adapter_name")?,
                        current: row.get::<_, Option<String>>("valid_to")?.is_none(),
                        t_created: row.get("t_created")?,
                        t_expired: row.get("t_expired")?,
                        edge_kind: row.get("edge_kind")?,
                        weight: row.get("weight")?,
                    })
                })?;
                for row in rows {
                    results.push(row?);
                }
            }
        }

        Ok(results)
    }

    pub fn read_triple_timestamps(
        &self,
        id: &str,
    ) -> anyhow::Result<(Option<String>, Option<String>)> {
        let result = self.conn.lock().unwrap().query_row(
            "SELECT t_created, t_expired FROM triples WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        );
        Ok(result?)
    }

    pub fn set_t_expired(&self, id: &str, value: Option<&str>) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE triples SET t_expired = ?1 WHERE id = ?2",
            params![value, id],
        )?;
        Ok(())
    }

    pub fn stats(&self) -> anyhow::Result<KgStats> {
        let total_entities: usize =
            self.conn
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM entities", [], |row| row.get(0))?;

        let total_triples: usize =
            self.conn
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM triples", [], |row| row.get(0))?;

        let current_facts: usize = self.conn.lock().unwrap().query_row(
            "SELECT COUNT(*) FROM triples WHERE valid_to IS NULL",
            [],
            |row| row.get(0),
        )?;

        let expired_facts = total_triples - current_facts;

        let _c = self.conn.lock().unwrap();
        let mut stmt = _c.prepare("SELECT DISTINCT predicate FROM triples ORDER BY predicate")?;
        let relationship_types: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(KgStats {
            total_entities,
            total_triples,
            current_facts,
            expired_facts,
            relationship_types,
        })
    }

    // -----------------------------------------------------------------------
    // mp-027 (issue #27): typed memory edges
    //
    // The KG already stores triples with arbitrary string predicates. The
    // typed-edge API below writes into dedicated `edge_kind` and `weight`
    // columns so callers can filter by edge kind (e.g. "find all HasTag
    // edges for cascade retrieval") without parsing the predicate.
    //
    // `add_memory_edge` is the single entry point for inserting a typed
    // edge. The `query_*_by_kind` helpers project the typed columns back
    // into a [`Triple`] for downstream consumers. The traversal weight is
    // recorded on the row so cascade retrievers can sort/filter by it
    // without re-deriving it from the variant.
    // -----------------------------------------------------------------------

    /// Add a typed memory edge between two entities. The traversal weight is
    /// taken from [`crate::types::MemoryEdgeKind::traversal_weight`] so
    /// cascade retrieval sees the canonical jcode weight.
    pub fn add_memory_edge(
        &self,
        from: &str,
        to: &str,
        kind: &crate::types::MemoryEdgeKind,
    ) -> anyhow::Result<String> {
        let kind_str = kind.as_str();
        let weight = kind.traversal_weight() as f64;
        let sub_id = Self::entity_id(from);
        let obj_id = Self::entity_id(to);

        // Ensure both endpoints exist in the entities table.
        self.conn.lock().unwrap().execute(
            "INSERT OR IGNORE INTO entities (id, name) VALUES (?1, ?2)",
            params![sub_id, from],
        )?;
        self.conn.lock().unwrap().execute(
            "INSERT OR IGNORE INTO entities (id, name) VALUES (?1, ?2)",
            params![obj_id, to],
        )?;

        // Deterministic triple ID: same (from, kind, to) always produces the
        // same ID, making add_memory_edge idempotent (matching jcode's
        // add_edge dedup behaviour).
        let triple_id = format!("me_{}_{}_{}", sub_id, kind_str, obj_id);

        // Check for existing edge (idempotent — skip if already present).
        let exists: bool = self.conn.lock().unwrap().query_row(
                "SELECT COUNT(*) > 0 FROM triples WHERE subject = ?1 AND edge_kind = ?2 AND object = ?3",
                params![sub_id, kind_str, obj_id],
                |row| row.get(0),
            )
            .unwrap_or(false);
        if exists {
            return Ok(triple_id);
        }

        let now = chrono::Utc::now().to_rfc3339();

        self.conn.lock().unwrap().execute(
            "INSERT INTO triples \
             (id, subject, predicate, object, valid_from, valid_to, confidence, \
              source_closet, source_file, source_drawer_id, adapter_name, \
              t_created, t_expired, edge_kind, weight) \
             VALUES (?1, ?2, ?3, ?4, NULL, NULL, ?5, NULL, NULL, NULL, NULL, \
                     ?6, NULL, ?7, ?8)",
            params![
                triple_id, sub_id, kind_str, obj_id,
                // confidence is independent of traversal weight; default to 1.0
                // because the typed-edge API is the source of truth for
                // weights.
                1.0_f64, now, kind_str, weight,
            ],
        )?;

        Ok(triple_id)
    }

    /// Query all triples that carry the given typed edge kind (current rows
    /// only; superseded rows are excluded by `valid_to IS NULL`).
    pub fn query_by_edge_kind(
        &self,
        kind: &crate::types::MemoryEdgeKind,
    ) -> anyhow::Result<Vec<Triple>> {
        let kind_str = kind.as_str();
        let _c = self.conn.lock().unwrap();
        let mut stmt = _c.prepare(
            "SELECT t.*, s.name as sub_name, o.name as obj_name FROM triples t \
             JOIN entities s ON t.subject = s.id \
             JOIN entities o ON t.object = o.id \
             WHERE t.edge_kind = ?1 AND t.valid_to IS NULL",
        )?;
        let rows = stmt.query_map(params![kind_str], |row| {
            // We know the predicate; pass it as the stored kind name so
            // round-trips preserve the typed view even when callers use the
            // generic `predicate` field.
            self.row_to_triple(row, kind_str)
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Outgoing edges of a given kind from `subject`. Mirrors
    /// [`Self::query_outgoing`] but restricts to a single typed edge kind.
    pub fn query_outgoing_by_kind(
        &self,
        subject: &str,
        kind: &crate::types::MemoryEdgeKind,
    ) -> anyhow::Result<Vec<Triple>> {
        let eid = Self::entity_id(subject);
        let kind_str = kind.as_str();
        let _c = self.conn.lock().unwrap();
        let mut stmt = _c.prepare(
            "SELECT t.*, s.name as sub_name, o.name as obj_name FROM triples t \
             JOIN entities s ON t.subject = s.id \
             JOIN entities o ON t.object = o.id \
             WHERE t.subject = ?1 AND t.edge_kind = ?2 AND t.valid_to IS NULL",
        )?;
        let rows = stmt.query_map(params![eid, kind_str], |row| {
            self.row_to_triple(row, kind_str)
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Incoming edges of a given kind to `object`. Mirrors
    /// [`Self::query_incoming`] but restricts to a single typed edge kind.
    pub fn query_incoming_by_kind(
        &self,
        object: &str,
        kind: &crate::types::MemoryEdgeKind,
    ) -> anyhow::Result<Vec<Triple>> {
        let eid = Self::entity_id(object);
        let kind_str = kind.as_str();
        let _c = self.conn.lock().unwrap();
        let mut stmt = _c.prepare(
            "SELECT t.*, s.name as sub_name, o.name as obj_name FROM triples t \
             JOIN entities s ON t.subject = s.id \
             JOIN entities o ON t.object = o.id \
             WHERE t.object = ?1 AND t.edge_kind = ?2 AND t.valid_to IS NULL",
        )?;
        let rows = stmt.query_map(params![eid, kind_str], |row| {
            self.row_to_triple(row, kind_str)
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Record a retrieval feedback outcome for a drawer.
    /// outcome: "helpful", "unhelpful", or "neutral"
    pub fn record_feedback(
        &self,
        drawer_id: &str,
        query: &str,
        outcome: &str,
    ) -> anyhow::Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO episodes (drawer_id, query, outcome) VALUES (?1, ?2, ?3)",
            params![drawer_id, query, outcome],
        )?;
        Ok(())
    }

    /// Get helpfulness score for a drawer based on historical feedback.
    /// Returns a multiplier between 0.5 (unhelpful) and 1.5 (helpful).
    pub fn helpfulness_score(&self, drawer_id: &str) -> anyhow::Result<f64> {
        let helpful: usize = self.conn.lock().unwrap().query_row(
            "SELECT COUNT(*) FROM episodes WHERE drawer_id = ?1 AND outcome = 'helpful'",
            params![drawer_id],
            |row| row.get(0),
        )?;
        let unhelpful: usize = self.conn.lock().unwrap().query_row(
            "SELECT COUNT(*) FROM episodes WHERE drawer_id = ?1 AND outcome = 'unhelpful'",
            params![drawer_id],
            |row| row.get(0),
        )?;
        let total = helpful + unhelpful;
        if total == 0 {
            return Ok(1.0); // No feedback = neutral
        }
        // Score: helpful ratio mapped to [0.5, 1.5]
        let ratio = helpful as f64 / total as f64;
        Ok(0.5 + ratio)
    }

    /// Get feedback history for a drawer.
    pub fn get_feedback(&self, drawer_id: &str) -> anyhow::Result<Vec<(String, String)>> {
        let _c = self.conn.lock().unwrap();
        let mut stmt = _c.prepare(
            "SELECT query, outcome FROM episodes WHERE drawer_id = ?1 ORDER BY feedback_at DESC LIMIT 50",
        )?;
        let rows = stmt.query_map(params![drawer_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // -----------------------------------------------------------------------
    // mp-034 (issue #34): cluster management
    //
    // Clusters group related memory drawers together. Each cluster has a
    // centroid embedding (average of member embeddings) and is linked to
    // its members via InCluster edges in the triples table.
    // -----------------------------------------------------------------------

    /// Serialize an `&[f32]` embedding into a little-endian byte blob for
    /// SQLite BLOB storage.
    pub(crate) fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
        embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    /// Deserialize a little-endian byte blob back into a `Vec<f32>`.
    pub(crate) fn blob_to_embedding(blob: &[u8]) -> Vec<f32> {
        blob.chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect()
    }

    /// Create a new cluster entry and link members via InCluster edges.
    ///
    /// If the cluster ID already exists, the entry is replaced (upsert).
    /// Each member gets an `InCluster` typed edge pointing at the cluster
    /// entity so `query_outgoing_by_kind(InCluster)` works without extra
    /// plumbing.
    pub fn create_cluster(
        &self,
        id: &str,
        name: Option<&str>,
        centroid: &[f32],
        members: &[String],
    ) -> anyhow::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let blob = Self::embedding_to_blob(centroid);

        self.conn.lock().unwrap().execute(
            "INSERT OR REPLACE INTO clusters (id, name, centroid, member_count, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![id, name, blob, members.len() as i64, now, now],
        )?;

        // Ensure the cluster itself exists as an entity so InCluster edges
        // have valid foreign keys.
        self.conn.lock().unwrap().execute(
            "INSERT OR IGNORE INTO entities (id, name) VALUES (?1, ?2)",
            rusqlite::params![id, name.unwrap_or(id)],
        )?;

        // Add InCluster edges from each member to the cluster.
        // Skip members that already have an InCluster edge to this cluster
        // (makes the operation idempotent for refine_clusters merges).
        let kind = crate::types::MemoryEdgeKind::InCluster;
        let kind_str = kind.as_str();
        for member_id in members {
            let sub_id = Self::entity_id(member_id);
            let existing: bool = self.conn.lock().unwrap().query_row(
                "SELECT COUNT(*) > 0 FROM triples \
                 WHERE subject = ?1 AND object = ?2 AND edge_kind = ?3 AND valid_to IS NULL",
                rusqlite::params![sub_id, id, kind_str],
                |row| row.get(0),
            )?;
            if !existing {
                self.add_memory_edge(member_id, id, &kind)?;
            }
        }

        Ok(())
    }

    /// Retrieve a cluster by ID.
    pub fn get_cluster(&self, id: &str) -> anyhow::Result<Option<ClusterEntry>> {
        let _c = self.conn.lock().unwrap();
        let mut stmt = _c.prepare(
            "SELECT id, name, centroid, member_count, created_at, updated_at \
             FROM clusters WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![id], |row| {
            let blob: Vec<u8> = row.get(2)?;
            Ok(ClusterEntry {
                id: row.get(0)?,
                name: row.get(1)?,
                centroid: Self::blob_to_embedding(&blob),
                member_count: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Get all member entity IDs that have an InCluster edge pointing at the
    /// given cluster.
    pub fn get_cluster_members(&self, cluster_id: &str) -> anyhow::Result<Vec<String>> {
        let kind_str = crate::types::MemoryEdgeKind::InCluster.as_str();
        let _c = self.conn.lock().unwrap();
        let mut stmt = _c.prepare(
            "SELECT s.name FROM triples t \
             JOIN entities s ON t.subject = s.id \
             WHERE t.object = ?1 AND t.edge_kind = ?2 AND t.valid_to IS NULL",
        )?;
        let rows = stmt.query_map(rusqlite::params![cluster_id, kind_str], |row| {
            row.get::<_, String>(0)
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Update the human-readable name of a cluster.
    pub fn update_cluster_name(&self, id: &str, name: &str) -> anyhow::Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let changed = self.conn.lock().unwrap().execute(
            "UPDATE clusters SET name = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![name, now, id],
        )?;
        if changed == 0 {
            anyhow::bail!("cluster not found: {}", id);
        }
        // Also update the entity name so it appears correctly in the KG.
        self.conn.lock().unwrap().execute(
            "UPDATE entities SET name = ?1 WHERE id = ?2",
            rusqlite::params![name, id],
        )?;
        Ok(())
    }

    /// List all clusters, ordered by creation time.
    pub fn list_clusters(&self) -> anyhow::Result<Vec<ClusterEntry>> {
        let _c = self.conn.lock().unwrap();
        let mut stmt = _c.prepare(
            "SELECT id, name, centroid, member_count, created_at, updated_at \
             FROM clusters ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let blob: Vec<u8> = row.get(2)?;
            Ok(ClusterEntry {
                id: row.get(0)?,
                name: row.get(1)?,
                centroid: Self::blob_to_embedding(&blob),
                member_count: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // -----------------------------------------------------------------------
    // Graph Snapshot System (v0.3.0, Q4)
    // -----------------------------------------------------------------------

    /// Compute the degree (number of incident triples) of a single entity.
    pub fn get_entity_degree(&self, name: &str) -> anyhow::Result<usize> {
        let eid = Self::entity_id(name);
        let degree: usize = self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM triples WHERE subject = ?1 OR object = ?1",
                rusqlite::params![eid],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(degree)
    }

    /// Build and persist a new graph snapshot from the current KG state.
    pub fn create_snapshot(&self) -> anyhow::Result<GraphSnapshot> {
        let total_nodes: usize =
            self.conn
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM entities", [], |row| row.get(0))?;
        let total_edges: usize =
            self.conn
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM triples", [], |row| row.get(0))?;
        let mut top_degrees = std::collections::HashMap::new();
        let _c = self.conn.lock().unwrap();
        let mut stmt = _c.prepare(
            "SELECT e.name, COUNT(*) as degree FROM entities e \
             JOIN triples t ON t.subject = e.id OR t.object = e.id \
             GROUP BY e.id ORDER BY degree DESC LIMIT 500",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
        })?;
        for row in rows {
            let (name, degree) = row?;
            top_degrees.insert(name, degree);
        }
        let snapshot_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let top_json = serde_json::to_string(&top_degrees)?;
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM graph_snapshots", [])?;
        self.conn.lock().unwrap().execute(
            "INSERT INTO graph_snapshots (snapshot_id, total_nodes, total_edges, top_degrees, created_at, reset_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
            rusqlite::params![snapshot_id, total_nodes, total_edges, top_json, now],
        )?;
        Ok(GraphSnapshot {
            snapshot_id,
            total_nodes,
            total_edges,
            top_degrees,
            created_at: now,
            reset_at: None,
        })
    }

    /// Read the current graph snapshot, if any.
    pub fn get_snapshot(&self) -> anyhow::Result<Option<GraphSnapshot>> {
        let result = self.conn.lock().unwrap().query_row(
            "SELECT snapshot_id, total_nodes, total_edges, top_degrees, created_at, reset_at \
             FROM graph_snapshots LIMIT 1",
            [],
            |row| {
                let top_json: String = row.get(3)?;
                let top_degrees: std::collections::HashMap<String, usize> =
                    serde_json::from_str(&top_json).unwrap_or_default();
                Ok(GraphSnapshot {
                    snapshot_id: row.get(0)?,
                    total_nodes: row.get(1)?,
                    total_edges: row.get(2)?,
                    top_degrees,
                    created_at: row.get(4)?,
                    reset_at: row.get(5)?,
                })
            },
        );
        match result {
            Ok(s) => Ok(Some(s)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Reset the knowledge graph: enumeration-free empty snapshot with resetAt.
    pub fn reset_snapshot(&self) -> anyhow::Result<GraphSnapshot> {
        let snapshot_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM graph_snapshots", [])?;
        self.conn.lock().unwrap().execute(
            "INSERT INTO graph_snapshots (snapshot_id, total_nodes, total_edges, top_degrees, created_at, reset_at) \
             VALUES (?1, 0, 0, '{}', ?2, ?3)",
            rusqlite::params![snapshot_id, now, now],
        )?;
        Ok(GraphSnapshot {
            snapshot_id,
            total_nodes: 0,
            total_edges: 0,
            top_degrees: std::collections::HashMap::new(),
            created_at: now.clone(),
            reset_at: Some(now),
        })
    }

    /// Pre-flight check: returns node count and whether a prior snapshot exists.
    pub fn snapshot_preflight(&self) -> anyhow::Result<SnapshotPreflight> {
        let total_nodes: usize =
            self.conn
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM entities", [], |row| row.get(0))?;
        let existing = self.get_snapshot()?;
        Ok(SnapshotPreflight {
            total_nodes,
            has_snapshot: existing.is_some(),
        })
    }
}

/// Result of a snapshot pre-flight check.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SnapshotPreflight {
    pub total_nodes: usize,
    pub has_snapshot: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_knowledge_graph_basic() {
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

        let eid = kg.add_entity("Max", "person", None).unwrap();
        assert!(eid.contains("max"));

        let triple_id = kg
            .add_triple(
                "Max",
                "child_of",
                "Alice",
                Some("2015-04-01"),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert!(triple_id.starts_with("t_"));

        let results = kg.query_entity("Max", None, None, "outgoing").unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].predicate, "child_of");
        assert_eq!(results[0].object, "Alice");

        let stats = kg.stats().unwrap();
        assert_eq!(stats.total_entities, 2);
        assert_eq!(stats.total_triples, 1);
        assert_eq!(stats.current_facts, 1);
    }

    #[test]
    fn test_invalidation() {
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

        kg.add_triple(
            "Max",
            "does",
            "swimming",
            Some("2025-01-01"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let results = kg.query_entity("Max", None, None, "outgoing").unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].current);

        kg.invalidate("Max", "does", "swimming", Some("2025-06-01"))
            .unwrap();
        let results = kg.query_entity("Max", None, None, "outgoing").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].predicate, "does");
        assert_eq!(results[0].object, "swimming");
        assert!(!results[0].current);
        assert_eq!(results[0].valid_to.as_deref(), Some("2025-06-01"));

        let as_of_after = kg
            .query_entity("Max", Some("2025-07-01"), None, "outgoing")
            .unwrap();
        assert!(as_of_after.is_empty());

        let as_of_before = kg
            .query_entity("Max", Some("2025-03-01"), None, "outgoing")
            .unwrap();
        assert_eq!(as_of_before.len(), 1);
        assert_eq!(as_of_before[0].predicate, "does");
        assert_eq!(as_of_before[0].object, "swimming");
    }

    #[test]
    fn test_temporal_filtering() {
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

        kg.add_triple(
            "Max",
            "child_of",
            "Alice",
            Some("2015-04-01"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        kg.add_triple(
            "Alice",
            "worried_about",
            "Max injury",
            Some("2026-01-01"),
            Some("2026-02-28"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let current = kg
            .query_entity("Alice", Some("2026-01-15"), None, "outgoing")
            .unwrap();
        assert!(!current.is_empty());

        let after = kg
            .query_entity("Alice", Some("2026-03-01"), None, "outgoing")
            .unwrap();
        assert!(after.is_empty());

        let all = kg.query_entity("Alice", None, None, "outgoing").unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].object, "Max injury");
        assert!(!all[0].current);
    }

    #[test]
    fn test_timeline() {
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

        kg.add_triple(
            "Max",
            "child_of",
            "Alice",
            Some("2015-04-01"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        kg.add_triple(
            "Max",
            "does",
            "swimming",
            Some("2025-01-01"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let timeline = kg.timeline(Some("Max")).unwrap();
        assert_eq!(timeline.len(), 2);
    }

    #[test]
    fn test_episode_feedback_scoring() {
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

        // No feedback = neutral score
        assert_eq!(kg.helpfulness_score("drawer_1").unwrap(), 1.0);

        // Helpful feedback
        kg.record_feedback("drawer_1", "test query", "helpful")
            .unwrap();
        assert!(kg.helpfulness_score("drawer_1").unwrap() > 1.0);

        // Unhelpful feedback
        kg.record_feedback("drawer_2", "test query", "unhelpful")
            .unwrap();
        assert!(kg.helpfulness_score("drawer_2").unwrap() < 1.0);

        // Mixed feedback
        kg.record_feedback("drawer_3", "query1", "helpful").unwrap();
        kg.record_feedback("drawer_3", "query2", "unhelpful")
            .unwrap();
        let score = kg.helpfulness_score("drawer_3").unwrap();
        assert!(score > 0.5 && score < 1.5);

        // Get feedback history
        let history = kg.get_feedback("drawer_1").unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].1, "helpful");
    }

    #[test]
    fn test_auto_resolve_conflicts() {
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

        // Add first triple
        kg.add_triple(
            "Alice",
            "works_at",
            "Acme",
            Some("2020-01-01"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Query should find Alice at Acme
        let results = kg.query_entity("Alice", None, None, "outgoing").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].object, "Acme");
        assert!(results[0].current);

        // Add conflicting triple (same subject+predicate, different object)
        kg.add_triple(
            "Alice",
            "works_at",
            "NewCo",
            Some("2023-01-01"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let jan_2022 = kg
            .query_entity("Alice", Some("2022-01-01"), None, "outgoing")
            .unwrap();
        assert_eq!(jan_2022.len(), 1);
        assert_eq!(jan_2022[0].object, "Acme");

        let jan_2024 = kg
            .query_entity("Alice", Some("2024-01-01"), None, "outgoing")
            .unwrap();
        assert_eq!(jan_2024.len(), 1);
        assert_eq!(jan_2024[0].object, "NewCo");
        assert!(jan_2024[0].current);

        // But timeline should show both
        let timeline = kg.timeline(Some("Alice")).unwrap();
        assert_eq!(timeline.len(), 2);

        let all = kg.query_entity("Alice", None, None, "outgoing").unwrap();
        assert_eq!(all.len(), 2);
        assert!(all
            .iter()
            .any(|triple| triple.object == "Acme" && !triple.current));
        assert!(all
            .iter()
            .any(|triple| triple.object == "NewCo" && triple.current));
    }

    #[test]
    fn test_query_relationship_without_as_of_returns_expired_and_current() {
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

        kg.add_triple(
            "Alice",
            "works_at",
            "Acme",
            Some("2020-01-01"),
            Some("2022-12-31"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        kg.add_triple(
            "Bob",
            "works_at",
            "NewCo",
            Some("2023-01-01"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let results = kg.query_relationship("works_at", None, None).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results
            .iter()
            .any(|triple| triple.object == "Acme" && !triple.current));
        assert!(results
            .iter()
            .any(|triple| triple.object == "NewCo" && triple.current));
    }

    #[test]
    fn test_add_triple_rejects_inverted_interval() {
        // #1214: a triple with valid_to < valid_from never satisfies the
        // temporal filter, so the row would be invisible to every query.
        // add_triple must reject it at write time instead of silently
        // accepting an unqueryable fact.
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
        let err = kg
            .add_triple(
                "Alice",
                "works_at",
                "Acme",
                Some("2026-12-31"),
                Some("2026-01-01"),
                None,
                None,
                None,
                None,
                None,
            )
            .expect_err("inverted interval must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("inverted interval"),
            "error message should mention inverted interval, got: {msg}"
        );
    }

    #[test]
    fn test_add_triple_allows_point_in_time_and_open_intervals() {
        // #1214: open intervals (only valid_from OR only valid_to) and
        // same-value point-in-time facts must remain accepted; only strict
        // inversion is rejected.
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
        kg.add_triple(
            "Alice",
            "born_on",
            "Earth",
            Some("2026-05-11"),
            Some("2026-05-11"),
            None,
            None,
            None,
            None,
            None,
        )
        .expect("point-in-time facts must be accepted");
        kg.add_triple(
            "Bob",
            "joined",
            "Cohort A",
            Some("2026-01-01"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("open-ended intervals must be accepted");
    }

    #[test]
    fn test_add_triple_canonicalizes_plus_zero_zero_to_z() {
        // mr-gvpc: add_triple must canonicalize `+00:00` → `Z` so KG TEXT
        // comparisons stay consistent regardless of which zero-offset form
        // the caller used.
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
        kg.add_triple(
            "Alice",
            "works_at",
            "Acme",
            Some("2026-05-11T00:00:00+00:00"),
            Some("2027-01-01T00:00:00+00:00"),
            None,
            None,
            None,
            None,
            None,
        )
        .expect("+00:00 offsets must be accepted and normalized");

        let triples = kg
            .query_entity("Alice", None, None, "both")
            .expect("query_entity");
        assert_eq!(triples.len(), 1);
        let stored = triples[0].valid_from.as_deref().unwrap();
        let stored_to = triples[0].valid_to.as_deref().unwrap();
        assert!(
            stored.ends_with('Z') && !stored.contains('+'),
            "valid_from must be canonical Z form, got {stored:?}"
        );
        assert!(
            stored_to.ends_with('Z') && !stored_to.contains('+'),
            "valid_to must be canonical Z form, got {stored_to:?}"
        );
    }

    #[test]
    fn test_add_triple_rejects_naive_datetime() {
        // mr-gvpc: naive datetimes (no offset) are ambiguous — they cannot be
        // compared meaningfully as UTC. add_triple must reject them at write
        // time so the KG TEXT column is never polluted with non-UTC values.
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
        let err = kg
            .add_triple(
                "Alice",
                "works_at",
                "Acme",
                Some("2026-05-11T00:00:00"),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect_err("naive datetime must be rejected");
        assert!(
            err.to_string().contains("valid_from"),
            "error should reference the field, got: {err}"
        );
    }

    #[test]
    fn test_add_triple_rejects_non_utc_offset() {
        // mr-gvpc: a non-UTC offset (e.g. +05:30) would shift the timestamp
        // and break TEXT-based as_of comparisons. add_triple must reject any
        // non-UTC offset, even when the value is well-formed.
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
        let err = kg
            .add_triple(
                "Bob",
                "lives_in",
                "Tokyo",
                Some("2026-05-11T00:00:00+09:00"),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect_err("non-UTC offset must be rejected");
        assert!(
            err.to_string().contains("valid_from"),
            "error should reference the field, got: {err}"
        );
    }

    #[test]
    fn test_add_triple_persists_source_drawer_and_adapter() {
        // #1314 / RFC 002 §5.5: adapter-supplied provenance (source_drawer_id +
        // adapter_name) must round-trip into the triples row, not get silently
        // dropped at the storage layer.
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
        let triple_id = kg
            .add_triple(
                "operating-verb",
                "candidate",
                "husbandry",
                Some("2026-04-28"),
                None,
                None,
                Some("closet-42"),
                Some("docs/decisions.md"),
                Some("drawer_abc123"),
                Some("text-adapter"),
            )
            .expect("add_triple with provenance should succeed");
        let (closet, file, drawer, adapter): (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = kg
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT source_closet, source_file, source_drawer_id, adapter_name \
                 FROM triples WHERE id = ?1",
                params![triple_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(closet.as_deref(), Some("closet-42"));
        assert_eq!(file.as_deref(), Some("docs/decisions.md"));
        assert_eq!(drawer.as_deref(), Some("drawer_abc123"));
        assert_eq!(adapter.as_deref(), Some("text-adapter"));
    }

    #[test]
    fn test_query_entity_exposes_source_drawer_id() {
        // #1314: callers reading from the KG (timeline / query_entity) must
        // see the source_drawer_id so they can navigate back to the drawer
        // that produced the triple.
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
        kg.add_triple(
            "Alice",
            "wrote",
            "design-doc",
            Some("2026-04-28"),
            None,
            None,
            None,
            None,
            Some("drawer_xyz"),
            None,
        )
        .unwrap();
        let outgoing = kg.query_entity("Alice", None, None, "outgoing").unwrap();
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].source_drawer_id.as_deref(), Some("drawer_xyz"));

        let timeline = kg.timeline(Some("Alice")).unwrap();
        assert_eq!(timeline.len(), 1);
        assert_eq!(timeline[0].source_drawer_id.as_deref(), Some("drawer_xyz"));
        assert_eq!(timeline[0].adapter_name, None);
    }

    #[test]
    fn test_migrate_schema_adds_missing_provenance_columns() {
        // #1314: legacy palaces created before source_drawer_id/adapter_name
        // existed must be migrated in-place on open() so add_triple keeps
        // working. We simulate "legacy" by creating a pre-migration triples
        // table directly and then opening it via KnowledgeGraph::open.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let db_path = tmp.path().to_path_buf();
        {
            let legacy = rusqlite::Connection::open(&db_path).unwrap();
            legacy
                .execute_batch(
                    "
                    CREATE TABLE entities (
                        id TEXT PRIMARY KEY,
                        name TEXT NOT NULL,
                        entity_type TEXT DEFAULT 'unknown',
                        properties TEXT DEFAULT '{}',
                        created_at TEXT DEFAULT CURRENT_TIMESTAMP
                    );
                    CREATE TABLE triples (
                        id TEXT PRIMARY KEY,
                        subject TEXT NOT NULL,
                        predicate TEXT NOT NULL,
                        object TEXT NOT NULL,
                        valid_from TEXT,
                        valid_to TEXT,
                        confidence REAL DEFAULT 1.0,
                        source_closet TEXT,
                        source_file TEXT,
                        extracted_at TEXT DEFAULT CURRENT_TIMESTAMP
                    );
                    ",
                )
                .unwrap();
        }

        // Opening through KnowledgeGraph must run the in-place migration and
        // expose the new columns to add_triple without re-creating the table.
        let kg = KnowledgeGraph::open(&db_path).unwrap();
        let names: Vec<String> = {
            let conn_lock = kg.conn.lock().unwrap();
            let mut stmt = conn_lock.prepare("PRAGMA table_info(triples)").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        assert!(
            names.contains(&"source_drawer_id".to_string()),
            "migration must add source_drawer_id, got columns: {names:?}"
        );
        assert!(
            names.contains(&"adapter_name".to_string()),
            "migration must add adapter_name, got columns: {names:?}"
        );

        // And add_triple must still write the new columns end-to-end.
        let triple_id = kg
            .add_triple(
                "Migrated",
                "has",
                "drawer",
                None,
                None,
                None,
                None,
                None,
                Some("drawer_migrated"),
                Some("legacy-adapter"),
            )
            .unwrap();
        let (drawer, adapter): (Option<String>, Option<String>) = kg
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT source_drawer_id, adapter_name FROM triples WHERE id = ?1",
                params![triple_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(drawer.as_deref(), Some("drawer_migrated"));
        assert_eq!(adapter.as_deref(), Some("legacy-adapter"));
    }

    // -----------------------------------------------------------------------
    // mp-027 (issue #27): typed memory edges.
    // -----------------------------------------------------------------------
    use crate::types::MemoryEdgeKind;

    #[test]
    fn test_typed_edge_round_trip() {
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
        let id = kg
            .add_memory_edge("alice", "rust", &MemoryEdgeKind::HasTag)
            .unwrap();
        assert!(!id.is_empty());

        let rows = kg.query_by_edge_kind(&MemoryEdgeKind::HasTag).unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.subject, "alice");
        assert_eq!(row.object, "rust");
        assert_eq!(row.edge_kind.as_deref(), Some("has_tag"));
        // weight is stored as f32 then read back as f64, so compare with
        // epsilon instead of exact equality.
        let w = row.weight.expect("weight must be set for HasTag");
        assert!((w - 0.8).abs() < 1e-6, "expected ~0.8, got {w}");
        assert!(row.current);
    }

    #[test]
    fn test_query_by_edge_kind_filters_correctly() {
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
        kg.add_memory_edge("alice", "rust", &MemoryEdgeKind::HasTag)
            .unwrap();
        kg.add_memory_edge("alice", "python", &MemoryEdgeKind::HasTag)
            .unwrap();
        kg.add_memory_edge("alice", "bob", &MemoryEdgeKind::RelatesTo { weight: 0.4 })
            .unwrap();
        kg.add_memory_edge("alice", "carol", &MemoryEdgeKind::Supersedes)
            .unwrap();

        let has_tag = kg.query_by_edge_kind(&MemoryEdgeKind::HasTag).unwrap();
        assert_eq!(has_tag.len(), 2);
        for row in &has_tag {
            assert_eq!(row.edge_kind.as_deref(), Some("has_tag"));
            let w = row.weight.expect("weight must be set");
            assert!((w - 0.8).abs() < 1e-6, "expected ~0.8, got {w}");
        }

        let supersedes = kg.query_by_edge_kind(&MemoryEdgeKind::Supersedes).unwrap();
        assert_eq!(supersedes.len(), 1);
        let w = supersedes[0].weight.expect("weight must be set");
        assert!((w - 0.9).abs() < 1e-6, "expected ~0.9, got {w}");

        let relates = kg
            .query_by_edge_kind(&MemoryEdgeKind::RelatesTo { weight: 0.4 })
            .unwrap();
        assert_eq!(relates.len(), 1);
        // The user-supplied weight round-trips through the column.
        let w = relates[0].weight.expect("weight must be set");
        assert!((w - 0.4).abs() < 1e-6, "expected ~0.4, got {w}");

        let contradicts = kg.query_by_edge_kind(&MemoryEdgeKind::Contradicts).unwrap();
        assert!(contradicts.is_empty());
    }

    #[test]
    fn test_query_outgoing_and_incoming_by_kind() {
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
        kg.add_memory_edge("alice", "rust", &MemoryEdgeKind::HasTag)
            .unwrap();
        kg.add_memory_edge("alice", "bob", &MemoryEdgeKind::RelatesTo { weight: 0.5 })
            .unwrap();
        // incoming HasTag (bob is tagged by carol)
        kg.add_memory_edge("carol", "bob", &MemoryEdgeKind::HasTag)
            .unwrap();

        let outgoing = kg
            .query_outgoing_by_kind("alice", &MemoryEdgeKind::HasTag)
            .unwrap();
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].object, "rust");

        let incoming = kg
            .query_incoming_by_kind("bob", &MemoryEdgeKind::HasTag)
            .unwrap();
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].subject, "carol");

        // Filtering by a kind with no rows returns an empty vec, not an error.
        let none = kg
            .query_outgoing_by_kind("alice", &MemoryEdgeKind::Contradicts)
            .unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn test_relates_to_weight_is_separate_from_confidence() {
        // The traversal weight lives in its own column and is independent
        // from `confidence`. The typed-edge API defaults confidence to 1.0.
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
        kg.add_memory_edge("a", "b", &MemoryEdgeKind::RelatesTo { weight: 0.42 })
            .unwrap();

        let (confidence, weight): (f64, f64) = kg
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT confidence, weight FROM triples WHERE edge_kind = 'relates_to'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!((weight - 0.42).abs() < 1e-6, "expected ~0.42, got {weight}");
        assert!(
            (confidence - 1.0).abs() < 1e-6,
            "expected ~1.0, got {confidence}"
        );
    }

    #[test]
    fn test_schema_migration_adds_typed_edge_columns_idempotently() {
        // Simulate a legacy palace with no edge_kind/weight columns.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let db_path = tmp.path().to_path_buf();
        {
            let legacy = rusqlite::Connection::open(&db_path).unwrap();
            legacy
                .execute_batch(
                    "CREATE TABLE entities (id TEXT PRIMARY KEY, name TEXT NOT NULL); \
                     CREATE TABLE triples (id TEXT PRIMARY KEY, subject TEXT NOT NULL, \
                         predicate TEXT NOT NULL, object TEXT NOT NULL, \
                         valid_from TEXT, valid_to TEXT, confidence REAL DEFAULT 1.0, \
                         source_closet TEXT, source_file TEXT, extracted_at TEXT);",
                )
                .unwrap();
        }

        // First open runs the migration.
        let kg = KnowledgeGraph::open(&db_path).unwrap();
        let names: Vec<String> = {
            let conn_lock = kg.conn.lock().unwrap();
            let mut stmt = conn_lock.prepare("PRAGMA table_info(triples)").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        assert!(names.contains(&"edge_kind".to_string()));
        assert!(names.contains(&"weight".to_string()));

        // Second open must be a no-op (idempotent).
        let kg2 = KnowledgeGraph::open(&db_path).unwrap();
        let names2: Vec<String> = {
            let conn_lock = kg2.conn.lock().unwrap();
            let mut stmt = conn_lock.prepare("PRAGMA table_info(triples)").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        assert_eq!(names, names2);
    }

    #[test]
    fn test_schema_migration_backfills_legacy_predicate_encodings() {
        // Simulate a palace that pre-dates typed edges and stored weights
        // inside the predicate string (the relations.rs convention).
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let db_path = tmp.path().to_path_buf();
        {
            let legacy = rusqlite::Connection::open(&db_path).unwrap();
            legacy
                .execute_batch(
                    "CREATE TABLE entities (id TEXT PRIMARY KEY, name TEXT NOT NULL); \
                     CREATE TABLE triples (id TEXT PRIMARY KEY, subject TEXT NOT NULL, \
                         predicate TEXT NOT NULL, object TEXT NOT NULL, \
                         valid_from TEXT, valid_to TEXT, confidence REAL DEFAULT 1.0, \
                         source_closet TEXT, source_file TEXT, extracted_at TEXT);",
                )
                .unwrap();
            legacy
                .execute(
                    "INSERT INTO triples (id, subject, predicate, object) VALUES ('e1','alice','relates_to_0.80','bob')",
                    [],
                )
                .unwrap();
            legacy
                .execute(
                    "INSERT INTO triples (id, subject, predicate, object) VALUES ('e2','bob','supersedes','carol')",
                    [],
                )
                .unwrap();
        }

        let kg = KnowledgeGraph::open(&db_path).unwrap();
        let conn_lock = kg.conn.lock().unwrap();
        let mut stmt = conn_lock
            .prepare("SELECT id, edge_kind, weight FROM triples ORDER BY id")
            .unwrap();
        let rows: Vec<(String, Option<String>, Option<f64>)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        // relates_to_0.80 → edge_kind=relates_to, weight=0.80
        assert_eq!(rows[0].0, "e1");
        assert_eq!(rows[0].1.as_deref(), Some("relates_to"));
        let w0 = rows[0].2.expect("relates_to must have weight");
        assert!((w0 - 0.80).abs() < 1e-6);

        // supersedes → edge_kind=supersedes, weight=0.9 (canonical)
        assert_eq!(rows[1].0, "e2");
        assert_eq!(rows[1].1.as_deref(), Some("supersedes"));
        let w1 = rows[1].2.expect("supersedes must have weight");
        assert!((w1 - 0.9).abs() < 1e-6);
    }
}

// =============================================================================
// Bitemporal query tests
// =============================================================================

#[cfg(test)]
mod bitemporal_tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Helper: parse an ISO-8601 timestamp string into a chrono DateTime
    // -------------------------------------------------------------------------
    fn parse_ts(s: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| {
                // Fallback for date-only strings like "2024-01-01"
                chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                    .unwrap()
                    .and_hms_opt(0, 0, 0)
                    .unwrap()
                    .and_utc()
            })
    }

    fn ts_within_last_5s(ts: &Option<String>) -> bool {
        let ts = ts
            .as_ref()
            .expect("t_created should not be NULL for newly added triple");
        let parsed = parse_ts(ts);
        let now = chrono::Utc::now();
        let diff = (now - parsed).num_seconds();
        (0..=5).contains(&diff)
    }

    // -------------------------------------------------------------------------
    // Test 1 – Migration backfill
    // -------------------------------------------------------------------------
    #[test]
    fn test_migration_backfill_sets_t_created_and_t_expired() {
        // Simulate a legacy palace where triples rows were inserted BEFORE
        // t_created/t_expired columns existed. After schema migration via
        // KnowledgeGraph::open, those existing rows should get:
        //   - t_created = COALESCE(valid_from, extracted_at)
        //   - t_expired = NULL
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let db_path = tmp.path().to_path_buf();
        {
            let legacy = rusqlite::Connection::open(&db_path).unwrap();
            legacy
                .execute_batch(
                    "
                    CREATE TABLE entities (
                        id TEXT PRIMARY KEY,
                        name TEXT NOT NULL,
                        entity_type TEXT DEFAULT 'unknown',
                        properties TEXT DEFAULT '{}',
                        created_at TEXT DEFAULT CURRENT_TIMESTAMP
                    );
                    CREATE TABLE triples (
                        id TEXT PRIMARY KEY,
                        subject TEXT NOT NULL,
                        predicate TEXT NOT NULL,
                        object TEXT NOT NULL,
                        valid_from TEXT,
                        valid_to TEXT,
                        confidence REAL DEFAULT 1.0,
                        source_closet TEXT,
                        source_file TEXT,
                        extracted_at TEXT DEFAULT CURRENT_TIMESTAMP
                    );
                    INSERT INTO entities (id, name) VALUES ('alice', 'Alice');
                    INSERT INTO entities (id, name) VALUES ('acme', 'Acme');
                    -- Row with valid_from set
                    INSERT INTO triples (id, subject, predicate, object, valid_from, extracted_at)
                    VALUES ('legacy_1', 'alice', 'works_at', 'acme', '2020-01-15', '2019-06-01');
                    -- Row with no valid_from, falls back to extracted_at
                    INSERT INTO triples (id, subject, predicate, object, valid_from, extracted_at)
                    VALUES ('legacy_2', 'alice', 'lives_in', 'acme', NULL, '2021-03-10');
                    -- Row with valid_from that should become t_created
                    INSERT INTO triples (id, subject, predicate, object, valid_from, extracted_at)
                    VALUES ('legacy_3', 'alice', 'visited', 'acme', '2022-07-01', '2022-06-15');
                    ",
                )
                .unwrap();
        }

        // Open through KnowledgeGraph — migration runs automatically
        let kg = KnowledgeGraph::open(&db_path).unwrap();

        // Row 1: valid_from = "2020-01-15" is more recent than extracted_at = "2019-06-01"
        // COALESCE(valid_from, extracted_at) = "2020-01-15"
        let (t_created, t_expired): (String, Option<String>) = kg
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT t_created, t_expired FROM triples WHERE id = 'legacy_1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            t_created, "2020-01-15",
            "t_created should backfill from valid_from"
        );
        assert!(
            t_expired.is_none(),
            "t_expired should be NULL for migrated row"
        );

        // Row 2: valid_from IS NULL → falls back to extracted_at = "2021-03-10"
        let (t_created, t_expired): (String, Option<String>) = kg
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT t_created, t_expired FROM triples WHERE id = 'legacy_2'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            t_created, "2021-03-10",
            "t_created should fall back to extracted_at"
        );
        assert!(
            t_expired.is_none(),
            "t_expired should be NULL for migrated row"
        );

        // Row 3: valid_from = "2022-07-01" is more recent than extracted_at = "2022-06-15"
        let (t_created, t_expired): (String, Option<String>) = kg
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT t_created, t_expired FROM triples WHERE id = 'legacy_3'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            t_created, "2022-07-01",
            "t_created should backfill from valid_from"
        );
        assert!(
            t_expired.is_none(),
            "t_expired should be NULL for migrated row"
        );
    }

    // -------------------------------------------------------------------------
    // Test 2 – New triple gets t_created=NOW and t_expired=NULL
    // -------------------------------------------------------------------------
    #[test]
    fn test_new_triple_sets_t_created_to_now_and_t_expired_null() {
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
        let id = kg
            .add_triple(
                "Bob",
                "employed_by",
                "TechCorp",
                Some("2025-01-01"),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();

        let (t_created, t_expired): (Option<String>, Option<String>) = kg
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT t_created, t_expired FROM triples WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert!(
            ts_within_last_5s(&t_created),
            "t_created should be within 5 seconds of now, got: {:?}",
            t_created
        );
        assert!(
            t_expired.is_none(),
            "t_expired should be NULL for newly added triple, got: {:?}",
            t_expired
        );
    }

    // -------------------------------------------------------------------------
    // Test 3 – Transaction_time query with tt_as_of
    // -------------------------------------------------------------------------
    #[test]
    fn test_tt_as_of_returns_correct_version_of_facts() {
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

        // Add Triple A (no valid_from — valid for all time)
        kg.add_triple(
            "Carol", "status", "active", None, None, None, None, None, None, None,
        )
        .unwrap();

        // Get the timestamp after adding A (but before adding B)
        let before_b: String = chrono::Utc::now().to_rfc3339();
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Add Triple B (same subject+predicate, different object — auto-conflict)
        kg.add_triple(
            "Carol",
            "status",
            "inactive",
            Some("2025-01-01"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Query with tt_as_of = before B was added → should only see A
        let results_before = kg
            .query_entity("Carol", None, Some(&before_b), "outgoing")
            .unwrap();
        assert_eq!(
            results_before.len(),
            1,
            "tt_as_of before B added should return 1 fact (A)"
        );
        assert_eq!(
            results_before[0].object, "active",
            "The fact should be 'active' (Triple A)"
        );
        assert!(
            results_before[0].t_expired.is_none(),
            "Triple A should not be expired yet"
        );

        // Query with tt_as_of = now (after B was added)
        let after_b: String = chrono::Utc::now().to_rfc3339();
        let results_after = kg
            .query_entity("Carol", None, Some(&after_b), "outgoing")
            .unwrap();
        // After auto-conflict resolution, only B is current; A got a valid_to set
        assert!(
            !results_after.is_empty(),
            "Should return at least 1 fact after B added"
        );
        let current = results_after
            .iter()
            .find(|r| r.object == "inactive" && r.current)
            .expect("Triple B should be current");
        assert!(
            current.t_created.is_some(),
            "Triple B should have t_created set"
        );
    }

    // -------------------------------------------------------------------------
    // Test 4 – Combined valid_time + transaction_time
    // -------------------------------------------------------------------------
    #[test]
    fn test_combined_valid_time_and_transaction_time_filter() {
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

        // Add fact that is valid 2020–2022
        kg.add_triple(
            "Dave",
            "worked_at",
            "AlphaInc",
            Some("2020-01-01"),
            Some("2022-12-31"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Get current transaction time
        let now = chrono::Utc::now().to_rfc3339();

        // Query valid_time at 2021 (mid-interval) with current transaction time
        let results = kg
            .query_entity("Dave", Some("2021-06-15"), Some(&now), "outgoing")
            .unwrap();
        assert_eq!(
            results.len(),
            1,
            "Should find AlphaInc for valid_time in 2021"
        );
        assert_eq!(results[0].object, "AlphaInc");

        // Query valid_time at 2023 (after interval ended) — should find nothing
        let results = kg
            .query_entity("Dave", Some("2023-01-01"), Some(&now), "outgoing")
            .unwrap();
        assert!(
            results.is_empty(),
            "Should find no facts for valid_time after interval ended"
        );

        // Query valid_time at 2019 (before interval started) — should find nothing
        let results = kg
            .query_entity("Dave", Some("2019-06-01"), Some(&now), "outgoing")
            .unwrap();
        assert!(
            results.is_empty(),
            "Should find no facts for valid_time before interval started"
        );
    }

    // -------------------------------------------------------------------------
    // Test 5 – t_expired supersedes fact in transaction_time queries
    // -------------------------------------------------------------------------
    #[test]
    fn test_t_expired_supersedes_fact_in_transaction_time_query() {
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

        // Add a triple
        let id = kg
            .add_triple(
                "Eve",
                "located_at",
                "BuildingA",
                Some("2024-01-01"),
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();

        // Manually set t_expired to a date in the future
        kg.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE triples SET t_expired = ?1 WHERE id = ?2",
                params!["2050-01-01", id],
            )
            .unwrap();

        let after_expired = "2099-01-01";
        let before_expired = "2030-01-01";

        // Query with tt_as_of after t_expired → should NOT return the fact
        let results = kg
            .query_entity("Eve", None, Some(after_expired), "outgoing")
            .unwrap();
        assert!(
            results.is_empty(),
            "Query at tt_as_of after t_expired should not return superseded fact"
        );

        // Query with tt_as_of before t_expired → should return the fact
        let results = kg
            .query_entity("Eve", None, Some(before_expired), "outgoing")
            .unwrap();
        assert_eq!(
            results.len(),
            1,
            "Query at tt_as_of before t_expired should return the fact"
        );
        assert_eq!(results[0].object, "BuildingA");
    }

    // -------------------------------------------------------------------------
    // Test 6 – tt_as_of combined with valid_time filter (both set)
    // -------------------------------------------------------------------------
    #[test]
    fn test_tt_as_of_with_valid_time_both_set() {
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

        // Fact valid 2020-2025, added at transaction_time T1
        kg.add_triple(
            "Frank",
            "deployed_to",
            "Cluster1",
            Some("2020-01-01"),
            Some("2025-12-31"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let t1 = chrono::Utc::now().to_rfc3339();
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Same subject/predicate, different object, valid 2026+
        kg.add_triple(
            "Frank",
            "deployed_to",
            "Cluster2",
            Some("2026-01-01"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Query valid_time=2024 (Cluster1 valid) + tt_as_of after T1 (Cluster2 exists)
        let results = kg
            .query_entity("Frank", Some("2024-06-15"), Some(&t1), "outgoing")
            .unwrap();
        // At 2024, Cluster1 is current, Cluster2 doesn't exist yet
        assert_eq!(
            results.len(),
            1,
            "Should only return Cluster1 for valid_time=2024, tt_as_of=T1"
        );
        assert_eq!(results[0].object, "Cluster1");

        // Query valid_time=2027 (Cluster2 valid) + tt_as_of after T1
        let results = kg
            .query_entity("Frank", Some("2027-01-15"), Some(&t1), "outgoing")
            .unwrap();
        assert!(
            results.is_empty(),
            "Cluster2 doesn't exist yet at tt_as_of=T1"
        );

        // Query valid_time=2027 with current transaction time → Cluster2
        let t2 = chrono::Utc::now().to_rfc3339();
        let results = kg
            .query_entity("Frank", Some("2027-01-15"), Some(&t2), "outgoing")
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].object, "Cluster2");
    }

    // -------------------------------------------------------------------------
    // Test 7 – t_expired column is NULL for non-superseded triples
    // -------------------------------------------------------------------------
    #[test]
    fn test_t_expired_is_null_for_current_triples() {
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
        kg.add_triple(
            "Grace",
            "member_of",
            "TeamAlpha",
            Some("2023-01-01"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let results = kg.query_entity("Grace", None, None, "outgoing").unwrap();
        assert_eq!(results.len(), 1);
        assert!(
            results[0].t_expired.is_none(),
            "Current (non-expired) triple should have t_expired = NULL"
        );
        assert!(
            results[0].t_created.is_some(),
            "Triple should have t_created set"
        );
        assert!(results[0].current, "Triple should be marked current");
    }

    // -------------------------------------------------------------------------
    // Test 8 – timeline reflects transaction_time supersession
    // -------------------------------------------------------------------------
    #[test]
    fn test_timeline_reflects_transaction_time_supersession() {
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

        // Add first fact
        kg.add_triple(
            "Henry",
            "role",
            "Engineer",
            Some("2022-01-01"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Get timestamp then add conflicting fact
        let t1 = chrono::Utc::now().to_rfc3339();
        std::thread::sleep(std::time::Duration::from_millis(100));

        kg.add_triple(
            "Henry",
            "role",
            "Senior Engineer",
            Some("2024-01-01"),
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Timeline should show both
        let timeline = kg.timeline(Some("Henry")).unwrap();
        assert_eq!(timeline.len(), 2, "Timeline should show both role versions");

        // At transaction time T1, only Engineer was present
        let timeline_t1 = kg
            .timeline_for_transaction_time(Some("Henry"), Some(&t1))
            .unwrap();
        assert_eq!(
            timeline_t1.len(),
            1,
            "Timeline at tt_as_of=T1 should show only Engineer"
        );
        assert_eq!(timeline_t1[0].object, "Engineer");

        // At current transaction time, only Senior Engineer is current
        let timeline_now = kg
            .timeline_for_transaction_time(Some("Henry"), None)
            .unwrap();
        assert_eq!(
            timeline_now.len(),
            1,
            "Timeline at current tt should show only Senior Engineer"
        );
        assert_eq!(timeline_now[0].object, "Senior Engineer");
    }

    // -------------------------------------------------------------------------
    // Test 9 – edge case: valid_time point falls exactly on t_expired boundary
    // -------------------------------------------------------------------------
    #[test]
    fn test_t_expired_boundary_exclusive_in_valid_time_query() {
        let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

        // Add fact with t_expired set to exactly 2024-06-01
        kg.add_triple(
            "Ivy",
            "project",
            "Alpha",
            Some("2020-01-01"),
            Some("2024-06-01"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        // Update t_expired to 2024-06-01 (manual override for this test)
        kg.conn.lock().unwrap().execute(
                "UPDATE triples SET t_expired = '2024-06-01' WHERE subject = 'ivy' AND object = 'alpha'",
                [],
            )
            .unwrap();

        // Query at valid_time exactly on t_expired boundary
        let results = kg
            .query_entity("Ivy", Some("2024-06-01"), None, "outgoing")
            .unwrap();
        // t_expired = 2024-06-01 means the fact was superseded AT that date
        // So a query at exactly that date should NOT include it (boundary is exclusive)
        assert!(
            results.is_empty(),
            "Fact with t_expired = query date should be excluded (boundary exclusive)"
        );

        // Query one day before t_expired — should include it
        let results = kg
            .query_entity("Ivy", Some("2024-05-31"), None, "outgoing")
            .unwrap();
        assert_eq!(
            results.len(),
            1,
            "Fact should be visible just before t_expired boundary"
        );
    }
}
