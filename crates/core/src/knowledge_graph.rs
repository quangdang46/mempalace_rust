use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub struct KnowledgeGraph {
    conn: Connection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Triple {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub confidence: Option<f64>,
    pub source_closet: Option<String>,
    pub source_file: Option<String>,
    pub current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: String,
    pub name: String,
    pub entity_type: String,
    pub properties: serde_json::Value,
}

#[derive(Debug)]
pub struct KgStats {
    pub total_entities: usize,
    pub total_triples: usize,
    pub current_facts: usize,
    pub expired_facts: usize,
    pub relationship_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityQueryResult {
    pub direction: String,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub confidence: Option<f64>,
    pub source_closet: Option<String>,
    pub current: bool,
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
                extracted_at TEXT DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (subject) REFERENCES entities(id),
                FOREIGN KEY (object) REFERENCES entities(id)
            );

            CREATE INDEX IF NOT EXISTS idx_triples_subject ON triples(subject);
            CREATE INDEX IF NOT EXISTS idx_triples_object ON triples(object);
            CREATE INDEX IF NOT EXISTS idx_triples_predicate ON triples(predicate);
            CREATE INDEX IF NOT EXISTS idx_triples_valid ON triples(valid_from, valid_to);

            CREATE TABLE IF NOT EXISTS episodes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                drawer_id TEXT NOT NULL,
                query TEXT NOT NULL,
                outcome TEXT NOT NULL CHECK(outcome IN ('helpful', 'unhelpful', 'neutral')),
                feedback_at TEXT DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_episodes_drawer ON episodes(drawer_id);
            CREATE INDEX IF NOT EXISTS idx_episodes_outcome ON episodes(outcome);
            ",
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
            "INSERT INTO triples (id, subject, predicate, object, valid_from, valid_to, confidence, source_closet, source_file)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                triple_id,
                sub_id,
                pred,
                obj_id,
                valid_from,
                valid_to,
                confidence.unwrap_or(1.0),
                source_closet,
                source_file
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
        direction: &str,
    ) -> anyhow::Result<Vec<EntityQueryResult>> {
        let eid = Self::entity_id(name);
        let mut results = Vec::new();

        if direction == "outgoing" || direction == "both" {
            results.extend(self.query_outgoing(&eid, as_of)?);
        }

        if direction == "incoming" || direction == "both" {
            results.extend(self.query_incoming(&eid, as_of)?);
        }

        Ok(results)
    }

    fn query_outgoing(
        &self,
        eid: &str,
        as_of: Option<&str>,
    ) -> anyhow::Result<Vec<EntityQueryResult>> {
        let mut results = Vec::new();

        if let Some(date) = as_of {
            let mut stmt = self.conn.prepare(
                "SELECT t.*, e.name as obj_name FROM triples t JOIN entities e ON t.object = e.id WHERE t.subject = ?1 AND (t.valid_from IS NULL OR t.valid_from <= ?2) AND (t.valid_to IS NULL OR t.valid_to >= ?3)"
            )?;
            let rows = stmt.query_map(params![eid, date, date], |row| {
                self.row_to_entity_result(row, "outgoing", eid)
            })?;
            for row in rows {
                results.push(row?);
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

        Ok(results)
    }

    fn query_incoming(
        &self,
        eid: &str,
        as_of: Option<&str>,
    ) -> anyhow::Result<Vec<EntityQueryResult>> {
        let mut results = Vec::new();

        if let Some(date) = as_of {
            let mut stmt = self.conn.prepare(
                "SELECT t.*, e.name as sub_name FROM triples t JOIN entities e ON t.subject = e.id WHERE t.object = ?1 AND (t.valid_from IS NULL OR t.valid_from <= ?2) AND (t.valid_to IS NULL OR t.valid_to >= ?3)"
            )?;
            let rows = stmt.query_map(params![eid, date, date], |row| {
                self.row_to_entity_result_incoming(row, "incoming", eid)
            })?;
            for row in rows {
                results.push(row?);
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
            current: valid_to.is_none(),
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
            current: valid_to.is_none(),
        })
    }

    pub fn query_relationship(
        &self,
        predicate: &str,
        as_of: Option<&str>,
    ) -> anyhow::Result<Vec<Triple>> {
        let pred = predicate.to_lowercase().replace(' ', "_");
        let mut results = Vec::new();

        if let Some(date) = as_of {
            let mut stmt = self.conn.prepare(
                "SELECT t.*, s.name as sub_name, o.name as obj_name FROM triples t JOIN entities s ON t.subject = s.id JOIN entities o ON t.object = o.id WHERE t.predicate = ?1 AND (t.valid_from IS NULL OR t.valid_from <= ?2) AND (t.valid_to IS NULL OR t.valid_to >= ?3)"
            )?;
            let rows = stmt.query_map(params![pred, date, date], |row| {
                self.row_to_triple(row, &pred)
            })?;
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
            current: valid_to.is_none(),
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
                    current: row.get::<_, Option<String>>("valid_to")?.is_none(),
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
                    current: row.get::<_, Option<String>>("valid_to")?.is_none(),
                })
            })?;
            for row in rows {
                results.push(row?);
            }
        }

        Ok(results)
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
        )
        .expect("open-ended intervals must be accepted");
    }
}
