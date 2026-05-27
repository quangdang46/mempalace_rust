use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;

// SAFETY: `KnowledgeGraph` is `Send + Sync` because all mutable access to
// `conn` goes through `&mut self` (Rust's normal borrow rules), SQLite
// serializes concurrent writes internally (WAL mode), and multiple readers
// are OK with SQLite's reader-writer lock. The caller must serialize concurrent
// access, which the Palace layer (the primary consumer) does via its own
// locking. This is the same safety contract as other SQLite wrappers like
// r2d2, sqlx, etc.
//
// If you add a new code path that accesses conn from a background thread
// without going through Palace's locking, you MUST add a mutex there.
pub struct KnowledgeGraph {
    conn: Connection,
}

// SAFETY: documented on the struct.
unsafe impl Send for KnowledgeGraph {}

// SAFETY: documented on the struct.
unsafe impl Sync for KnowledgeGraph {}

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
}

impl KnowledgeGraph {
    pub fn open(db_path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;
        // Enable WAL mode for better concurrent read performance and reduced SQLITE_BUSY risk
        let _: String = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
        let kg = Self { conn };
        kg.init_db()?;
        Ok(kg)
    }

    fn init_db(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(
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
                FOREIGN KEY (subject) REFERENCES entities(id),
                FOREIGN KEY (object) REFERENCES entities(id)
            );

CREATE INDEX IF NOT EXISTS idx_triples_subject ON triples(subject);
            CREATE INDEX IF NOT EXISTS idx_triples_object ON triples(object);
            CREATE INDEX IF NOT EXISTS idx_triples_predicate ON triples(predicate);
            CREATE INDEX IF NOT EXISTS idx_triples_valid ON triples(valid_from, valid_to);
            ",
        )?;
        self.migrate_schema()?;
        Ok(())
    }

    /// Backwards-compatible schema migration for older `triples` tables
    /// (#1314 RFC 002 §5.5). Fresh palaces already have `source_drawer_id`
    /// and `adapter_name` from the `CREATE TABLE` above, so this is a no-op.
    /// Palaces created before those columns were added must be migrated in
    /// place — SQLite has no `ADD COLUMN IF NOT EXISTS`, so we introspect
    /// the schema and only issue the ALTER when the column is missing.
    fn migrate_schema(&self) -> anyhow::Result<()> {
        let mut stmt = self.conn.prepare("PRAGMA table_info(triples)")?;
        let names: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if !names.iter().any(|n| n == "source_drawer_id") {
            self.conn
                .execute("ALTER TABLE triples ADD COLUMN source_drawer_id TEXT", [])?;
        }
        if !names.iter().any(|n| n == "adapter_name") {
            self.conn
                .execute("ALTER TABLE triples ADD COLUMN adapter_name TEXT", [])?;
        }
        if !names.iter().any(|n| n == "t_created") {
            self.conn.execute(
                "ALTER TABLE triples ADD COLUMN t_created TEXT NOT NULL DEFAULT (datetime('now'))",
                [],
            )?;
        }
        if !names.iter().any(|n| n == "t_expired") {
            self.conn
                .execute("ALTER TABLE triples ADD COLUMN t_expired TEXT", [])?;
        }
        // Backfill existing rows that lack t_created
        self.conn.execute(
            "UPDATE triples SET t_created = COALESCE(valid_from, extracted_at) WHERE t_created IS NULL",
            [],
        )?;
        self.conn.execute(
            "UPDATE triples SET t_expired = NULL WHERE t_expired IS NULL",
            [],
        )?;
        Ok(())
    }

    fn entity_id(name: &str) -> String {
        name.to_lowercase().replace(' ', "_").replace('\'', "")
    }

    pub fn add_entity(
        &mut self,
        name: &str,
        entity_type: &str,
        properties: Option<&serde_json::Value>,
    ) -> anyhow::Result<String> {
        let eid = Self::entity_id(name);
        let props = match properties {
            Some(p) => serde_json::to_string(p)?,
            None => "{}".to_string(),
        };
        self.conn.execute(
            "INSERT OR REPLACE INTO entities (id, name, entity_type, properties) VALUES (?1, ?2, ?3, ?4)",
            params![eid, name, entity_type, props],
        )?;
        Ok(eid)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_triple(
        &mut self,
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
        // Reject inverted intervals (#1214): a triple with valid_to < valid_from
        // would never satisfy `valid_from <= as_of AND valid_to >= as_of`, so it
        // would be invisible to every query — silently corrupt. Open intervals
        // and point-in-time facts (valid_from == valid_to) remain accepted.
        if let (Some(vf), Some(vt)) = (valid_from, valid_to) {
            if vt < vf {
                anyhow::bail!(
                    "valid_to={vt:?} is before valid_from={vf:?}; an inverted interval would be invisible to every KG query"
                );
            }
        }
        let sub_id = Self::entity_id(subject);
        let obj_id = Self::entity_id(object);
        let pred = predicate.to_lowercase().replace(' ', "_");

        self.conn.execute(
            "INSERT OR IGNORE INTO entities (id, name) VALUES (?1, ?2)",
            params![sub_id, subject],
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO entities (id, name) VALUES (?1, ?2)",
            params![obj_id, object],
        )?;

        let check_exists: Result<String, _> = self.conn.query_row(
            "SELECT id FROM triples WHERE subject=?1 AND predicate=?2 AND object=?3 AND valid_to IS NULL",
            params![sub_id, pred, obj_id],
            |row| row.get(0),
        );

        if let Ok(existing_id) = check_exists {
            return Ok(existing_id);
        }

        // Auto-resolve conflicts: if same subject+predicate has different object,
        // invalidate the old triple first
        let conflicting: Result<String, _> = self.conn.query_row(
            "SELECT id FROM triples WHERE subject=?1 AND predicate=?2 AND valid_to IS NULL AND object<>?3",
            params![sub_id, pred, obj_id],
            |row| row.get(0),
        );

        if let Ok(conflict_id) = conflicting {
            // Invalidate the conflicting triple at the start of the new fact when known,
            // otherwise fall back to the current timestamp.
            let conflict_end = valid_from
                .map(|s| s.to_string())
                .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
            self.conn.execute(
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

        self.conn.execute(
            "INSERT INTO triples (id, subject, predicate, object, valid_from, valid_to, confidence, source_closet, source_file, source_drawer_id, adapter_name, t_created, t_expired)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
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
                Option::<String>::None
            ],
        )?;

        Ok(triple_id)
    }

    pub fn invalidate(
        &mut self,
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

        self.conn.execute(
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
                let mut stmt = self.conn.prepare(
                    "SELECT t.*, e.name as obj_name FROM triples t JOIN entities e ON t.object = e.id WHERE t.subject = ?1 AND (t.valid_from IS NULL OR t.valid_from <= ?2) AND (t.valid_to IS NULL OR t.valid_to >= ?3) AND (t.t_created IS NULL OR t.t_created <= ?4) AND (t.t_expired IS NULL OR t.t_expired >= ?5)"
                )?;
                let mut rows = stmt.query(params![eid, date, date, tt, tt])?;
                while let Some(row) = rows.next()? {
                    results.push(self.row_to_entity_result(row, "outgoing", eid)?);
                }
            } else {
                let mut stmt = self.conn.prepare(
                    "SELECT t.*, e.name as obj_name FROM triples t JOIN entities e ON t.object = e.id WHERE t.subject = ?1 AND (t.valid_from IS NULL OR t.valid_from <= ?2) AND (t.valid_to IS NULL OR t.valid_to >= ?3)"
                )?;
                let mut rows = stmt.query(params![eid, date, date])?;
                while let Some(row) = rows.next()? {
                    results.push(self.row_to_entity_result(row, "outgoing", eid)?);
                }
            }
        } else {
            if let Some(tt) = tt_as_of {
                let mut stmt = self.conn.prepare(
                    "SELECT t.*, e.name as obj_name FROM triples t JOIN entities e ON t.object = e.id WHERE t.subject = ?1 AND (t.t_created IS NULL OR t.t_created <= ?2) AND (t.t_expired IS NULL OR t.t_expired >= ?3)"
                )?;
                let mut rows = stmt.query(params![eid, tt, tt])?;
                while let Some(row) = rows.next()? {
                    results.push(self.row_to_entity_result(row, "outgoing", eid)?);
                }
            } else {
                let mut stmt = self.conn.prepare(
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
                let mut stmt = self.conn.prepare(
                    "SELECT t.*, e.name as sub_name FROM triples t JOIN entities e ON t.subject = e.id WHERE t.object = ?1 AND (t.valid_from IS NULL OR t.valid_from <= ?2) AND (t.valid_to IS NULL OR t.valid_to >= ?3) AND (t.t_created IS NULL OR t.t_created <= ?4) AND (t.t_expired IS NULL OR t.t_expired >= ?5)"
                )?;
                let mut rows = stmt.query(params![eid, date, date, tt, tt])?;
                while let Some(row) = rows.next()? {
                    results.push(self.row_to_entity_result_incoming(row, "incoming", eid)?);
                }
            } else {
                let mut stmt = self.conn.prepare(
                    "SELECT t.*, e.name as sub_name FROM triples t JOIN entities e ON t.subject = e.id WHERE t.object = ?1 AND (t.valid_from IS NULL OR t.valid_from <= ?2) AND (t.valid_to IS NULL OR t.valid_to >= ?3)"
                )?;
                let mut rows = stmt.query(params![eid, date, date])?;
                while let Some(row) = rows.next()? {
                    results.push(self.row_to_entity_result_incoming(row, "incoming", eid)?);
                }
            }
        } else {
            if let Some(tt) = tt_as_of {
                let mut stmt = self.conn.prepare(
                    "SELECT t.*, e.name as sub_name FROM triples t JOIN entities e ON t.subject = e.id WHERE t.object = ?1 AND (t.t_created IS NULL OR t.t_created <= ?2) AND (t.t_expired IS NULL OR t.t_expired >= ?3)"
                )?;
                let mut rows = stmt.query(params![eid, tt, tt])?;
                while let Some(row) = rows.next()? {
                    results.push(self.row_to_entity_result_incoming(row, "incoming", eid)?);
                }
            } else {
                let mut stmt = self.conn.prepare(
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
                let mut stmt = self.conn.prepare(
                    "SELECT t.*, s.name as sub_name, o.name as obj_name FROM triples t JOIN entities s ON t.subject = s.id JOIN entities o ON t.object = o.id WHERE t.predicate = ?1 AND (t.valid_from IS NULL OR t.valid_from <= ?2) AND (t.valid_to IS NULL OR t.valid_to >= ?3) AND (t.t_created IS NULL OR t.t_created <= ?4) AND (t.t_expired IS NULL OR t.t_expired >= ?5)"
                )?;
                let rows = stmt.query_map(params![pred, date, date, tt, tt], |row| {
                    self.row_to_triple(row, &pred)
                })?;
                for row in rows {
                    results.push(row?);
                }
            } else {
                let mut stmt = self.conn.prepare(
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
                let mut stmt = self.conn.prepare(
                    "SELECT t.*, s.name as sub_name, o.name as obj_name FROM triples t JOIN entities s ON t.subject = s.id JOIN entities o ON t.object = o.id WHERE t.predicate = ?1 AND (t.t_created IS NULL OR t.t_created <= ?2) AND (t.t_expired IS NULL OR t.t_expired >= ?3)"
                )?;
                let rows =
                    stmt.query_map(params![pred, tt, tt], |row| self.row_to_triple(row, &pred))?;
                for row in rows {
                    results.push(row?);
                }
            } else {
                let mut stmt = self.conn.prepare(
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
        })
    }

    pub fn timeline(&self, entity_name: Option<&str>) -> anyhow::Result<Vec<Triple>> {
        let mut results = Vec::new();

        if let Some(name) = entity_name {
            let eid = Self::entity_id(name);
            let mut stmt = self.conn.prepare(
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
                })
            })?;
            for row in rows {
                results.push(row?);
            }
        } else {
            let mut stmt = self.conn.prepare(
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
                let mut stmt = self.conn.prepare(
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
                    })
                })?;
                for row in rows {
                    results.push(row?);
                }
            } else {
                results.extend(self.timeline(Some(name))?);
            }
        } else if let Some(tt) = tt_as_of {
            let mut stmt = self.conn.prepare(
                "SELECT t.*, s.name as sub_name, o.name as obj_name FROM triples t \
                 JOIN entities s ON t.subject = s.id \
                 JOIN entities o ON t.object = o.id \
                 WHERE (t.t_created IS NULL OR t.t_created <= ?1) \
                 AND (t.t_expired IS NULL OR t.t_expired >= ?2) \
                 ORDER BY t.valid_from ASC LIMIT 100",
            )?;
            let rows = stmt.query_map(params![tt, tt], |row| {
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
                })
            })?;
            for row in rows {
                results.push(row?);
            }
        } else {
            results.extend(self.timeline(None)?);
        }

        Ok(results)
    }

    pub fn read_triple_timestamps(
        &self,
        id: &str,
    ) -> anyhow::Result<(Option<String>, Option<String>)> {
        let result = self.conn.query_row(
            "SELECT t_created, t_expired FROM triples WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        );
        Ok(result?)
    }

    pub fn set_t_expired(&self, id: &str, value: Option<&str>) -> anyhow::Result<()> {
        self.conn.execute(
            "UPDATE triples SET t_expired = ?1 WHERE id = ?2",
            params![value, id],
        )?;
        Ok(())
    }

    pub fn stats(&self) -> anyhow::Result<KgStats> {
        let total_entities: usize =
            self.conn
                .query_row("SELECT COUNT(*) FROM entities", [], |row| row.get(0))?;

        let total_triples: usize =
            self.conn
                .query_row("SELECT COUNT(*) FROM triples", [], |row| row.get(0))?;

        let current_facts: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM triples WHERE valid_to IS NULL",
            [],
            |row| row.get(0),
        )?;

        let expired_facts = total_triples - current_facts;

        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT predicate FROM triples ORDER BY predicate")?;
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

    /// Record a retrieval feedback outcome for a drawer.
    /// outcome: "helpful", "unhelpful", or "neutral"
    pub fn record_feedback(
        &self,
        drawer_id: &str,
        query: &str,
        outcome: &str,
    ) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO episodes (drawer_id, query, outcome) VALUES (?1, ?2, ?3)",
            params![drawer_id, query, outcome],
        )?;
        Ok(())
    }

    /// Get helpfulness score for a drawer based on historical feedback.
    /// Returns a multiplier between 0.5 (unhelpful) and 1.5 (helpful).
    pub fn helpfulness_score(&self, drawer_id: &str) -> anyhow::Result<f64> {
        let helpful: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM episodes WHERE drawer_id = ?1 AND outcome = 'helpful'",
            params![drawer_id],
            |row| row.get(0),
        )?;
        let unhelpful: usize = self.conn.query_row(
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
        let mut stmt = self.conn.prepare(
            "SELECT query, outcome FROM episodes WHERE drawer_id = ?1 ORDER BY feedback_at DESC LIMIT 50",
        )?;
        let rows = stmt.query_map(params![drawer_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_knowledge_graph_basic() {
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

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

        let results = kg.query_entity("Max", None, "outgoing").unwrap();
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
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

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
        let results = kg.query_entity("Max", None, "outgoing").unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].current);

        kg.invalidate("Max", "does", "swimming", Some("2025-06-01"))
            .unwrap();
        let results = kg.query_entity("Max", None, "outgoing").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].predicate, "does");
        assert_eq!(results[0].object, "swimming");
        assert!(!results[0].current);
        assert_eq!(results[0].valid_to.as_deref(), Some("2025-06-01"));

        let as_of_after = kg
            .query_entity("Max", Some("2025-07-01"), "outgoing")
            .unwrap();
        assert!(as_of_after.is_empty());

        let as_of_before = kg
            .query_entity("Max", Some("2025-03-01"), "outgoing")
            .unwrap();
        assert_eq!(as_of_before.len(), 1);
        assert_eq!(as_of_before[0].predicate, "does");
        assert_eq!(as_of_before[0].object, "swimming");
    }

    #[test]
    fn test_temporal_filtering() {
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

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
            Some("2026-01"),
            Some("2026-02"),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let current = kg
            .query_entity("Alice", Some("2026-01-15"), "outgoing")
            .unwrap();
        assert!(!current.is_empty());

        let after = kg
            .query_entity("Alice", Some("2026-03-01"), "outgoing")
            .unwrap();
        assert!(after.is_empty());

        let all = kg.query_entity("Alice", None, "outgoing").unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].object, "Max injury");
        assert!(!all[0].current);
    }

    #[test]
    fn test_timeline() {
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

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
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

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
        let results = kg.query_entity("Alice", None, "outgoing").unwrap();
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
            .query_entity("Alice", Some("2022-01-01"), "outgoing")
            .unwrap();
        assert_eq!(jan_2022.len(), 1);
        assert_eq!(jan_2022[0].object, "Acme");

        let jan_2024 = kg
            .query_entity("Alice", Some("2024-01-01"), "outgoing")
            .unwrap();
        assert_eq!(jan_2024.len(), 1);
        assert_eq!(jan_2024[0].object, "NewCo");
        assert!(jan_2024[0].current);

        // But timeline should show both
        let timeline = kg.timeline(Some("Alice")).unwrap();
        assert_eq!(timeline.len(), 2);

        let all = kg.query_entity("Alice", None, "outgoing").unwrap();
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
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

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

        let results = kg.query_relationship("works_at", None).unwrap();
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
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
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
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
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
    fn test_add_triple_persists_source_drawer_and_adapter() {
        // #1314 / RFC 002 §5.5: adapter-supplied provenance (source_drawer_id +
        // adapter_name) must round-trip into the triples row, not get silently
        // dropped at the storage layer.
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
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
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
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
        let outgoing = kg.query_entity("Alice", None, "outgoing").unwrap();
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
        let mut kg = KnowledgeGraph::open(&db_path).unwrap();
        let names: Vec<String> = {
            let mut stmt = kg.conn.prepare("PRAGMA table_info(triples)").unwrap();
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
            .query_row(
                "SELECT source_drawer_id, adapter_name FROM triples WHERE id = ?1",
                params![triple_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(drawer.as_deref(), Some("drawer_migrated"));
        assert_eq!(adapter.as_deref(), Some("legacy-adapter"));
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
        diff >= 0 && diff <= 5
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
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
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
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

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
            results_after.len() >= 1,
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
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

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
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

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

        // Manually set t_expired to a date in the past
        kg.conn
            .execute(
                "UPDATE triples SET t_expired = ?1 WHERE id = ?2",
                params!["2025-01-01", id],
            )
            .unwrap();

        let after_expired = "2025-06-01";
        let before_expired = "2024-06-01";

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
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

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
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
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

        let results = kg.query_entity("Grace", None, "outgoing").unwrap();
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
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

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
        let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();

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
        kg.conn
            .execute(
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
