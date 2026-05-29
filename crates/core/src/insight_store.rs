use crate::types::Insight;
use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};

pub struct InsightStore {
    conn: Connection,
}

impl InsightStore {
    pub fn new(conn: Connection) -> Result<Self> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS insights (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                content TEXT NOT NULL,
                confidence REAL NOT NULL DEFAULT 0.5,
                reinforcements INTEGER NOT NULL DEFAULT 0,
                source_observation_id TEXT,
                source_concept_cluster TEXT,
                source_memory_ids TEXT NOT NULL DEFAULT '[]',
                source_lesson_ids TEXT NOT NULL DEFAULT '[]',
                source_crystal_ids TEXT NOT NULL DEFAULT '[]',
                project TEXT,
                tags TEXT NOT NULL DEFAULT '[]',
                decay_rate REAL NOT NULL DEFAULT 0.05,
                last_reinforced_at TEXT,
                last_decayed_at TEXT,
                updated_at TEXT NOT NULL,
                created_at TEXT NOT NULL,
                deleted INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )?;
        Ok(Self { conn })
    }

    pub fn list(&self, project: Option<&str>, min_confidence: f64, limit: usize) -> Result<Vec<Insight>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, content, confidence, reinforcements,
                    source_observation_id, source_concept_cluster,
                    source_memory_ids, source_lesson_ids, source_crystal_ids,
                    project, tags, decay_rate, last_reinforced_at, last_decayed_at,
                    updated_at, created_at, deleted
             FROM insights WHERE deleted = 0 AND confidence >= ?1
             ORDER BY confidence DESC LIMIT ?2",
        )?;

        let insights: Vec<Insight> = stmt
            .query_map(params![min_confidence, limit], row_to_insight)?
            .filter_map(|r| r.ok())
            .collect();

        if let Some(proj) = project {
            Ok(insights.into_iter().filter(|i| i.project.as_deref() == Some(proj)).take(limit).collect())
        } else {
            Ok(insights)
        }
    }

    pub fn search(&self, query: &str, project: Option<&str>, min_confidence: f64, limit: usize) -> Result<Vec<(Insight, f64)>> {
        let insights = self.list(project, min_confidence, 1000)?;
        let terms: Vec<&str> = query.split_whitespace().filter(|t| t.len() > 1).collect();

        let mut scored: Vec<(Insight, f64)> = insights
            .into_iter()
            .filter_map(|i| {
                let text = format!("{} {} {}", i.title, i.content, i.tags.join(" ")).to_lowercase();
                let match_count = terms.iter().filter(|t| text.contains(*t)).count();
                if match_count == 0 { return None; }
                let relevance = match_count as f64 / terms.len() as f64;
                let days_since = i.last_reinforced_at.map(|t| (Utc::now() - t).num_days() as f64).unwrap_or(0.0);
                let recency_boost = 1.0 / (1.0 + days_since * 0.01);
                let score = i.confidence * relevance * recency_boost;
                Some((i, score))
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        Ok(scored)
    }

    pub fn decay_sweep(&self) -> Result<(usize, usize, usize)> {
        let insights = self.list(None, 0.0, 10000)?;
        let now = Utc::now();
        let mut decayed = 0;
        let mut soft_deleted = 0;
        let total = insights.len();

        for insight in insights {
            let baseline = insight.last_decayed_at.or(insight.last_reinforced_at).unwrap_or(insight.created_at);
            let weeks = (now - baseline).num_weeks() as f64;
            if weeks < 1.0 { continue; }

            let decay = insight.decay_rate * weeks;
            let new_confidence = (insight.confidence - decay).max(0.05);

            if (new_confidence - insight.confidence).abs() > 0.001 {
                let mut updated_insight = insight;
                updated_insight.confidence = new_confidence;
                updated_insight.last_decayed_at = Some(now);
                updated_insight.updated_at = now;

                if updated_insight.confidence <= 0.1 && updated_insight.reinforcements == 0 {
                    updated_insight.deleted = true;
                    soft_deleted += 1;
                } else {
                    decayed += 1;
                }
                self.update(&updated_insight)?;
            }
        }

        Ok((decayed, soft_deleted, total))
    }

    fn update(&self, insight: &Insight) -> Result<()> {
        self.conn.execute(
            "UPDATE insights SET title=?2, content=?3, confidence=?4, reinforcements=?5,
                                source_observation_id=?6, source_concept_cluster=?7,
                                source_memory_ids=?8, source_lesson_ids=?9, source_crystal_ids=?10,
                                project=?11, tags=?12, decay_rate=?13,
                                last_reinforced_at=?14, last_decayed_at=?15,
                                updated_at=?16, deleted=?17 WHERE id=?1",
            params![
                insight.id, insight.title, insight.content, insight.confidence, insight.reinforcements,
                insight.source_observation_id, insight.source_concept_cluster,
                serde_json::to_string(&insight.source_memory_ids)?,
                serde_json::to_string(&insight.source_lesson_ids)?,
                serde_json::to_string(&insight.source_crystal_ids)?,
                insight.project, serde_json::to_string(&insight.tags)?,
                insight.decay_rate,
                insight.last_reinforced_at.map(|d| d.to_rfc3339()),
                insight.last_decayed_at.map(|d| d.to_rfc3339()),
                insight.updated_at.to_rfc3339(), insight.deleted as i32
            ],
        )?;
        Ok(())
    }
}

fn row_to_insight(row: &rusqlite::Row<'_>) -> rusqlite::Result<Insight> {
    let source_memory_ids: String = row.get(7)?;
    let source_lesson_ids: String = row.get(8)?;
    let source_crystal_ids: String = row.get(9)?;
    let tags: String = row.get(11)?;
    Ok(Insight {
        id: row.get(0)?,
        title: row.get(1)?,
        content: row.get(2)?,
        confidence: row.get(3)?,
        reinforcements: row.get(4)?,
        source_observation_id: row.get(5)?,
        source_concept_cluster: row.get(6)?,
        source_memory_ids: serde_json::from_str(&source_memory_ids).unwrap_or_default(),
        source_lesson_ids: serde_json::from_str(&source_lesson_ids).unwrap_or_default(),
        source_crystal_ids: serde_json::from_str(&source_crystal_ids).unwrap_or_default(),
        project: row.get(10)?,
        tags: serde_json::from_str(&tags).unwrap_or_default(),
        decay_rate: row.get(12)?,
        last_reinforced_at: row.get::<_, Option<String>>(13)?.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok()).map(|dt| dt.with_timezone(&Utc)),
        last_decayed_at: row.get::<_, Option<String>>(14)?.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok()).map(|dt| dt.with_timezone(&Utc)),
        updated_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(15)?).unwrap().with_timezone(&Utc),
        created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(16)?).unwrap().with_timezone(&Utc),
        deleted: row.get::<_, i32>(17)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> InsightStore {
        InsightStore::new(Connection::open_in_memory().unwrap()).unwrap()
    }

    fn insert_test_insight(store: &InsightStore, id: &str, title: &str, content: &str, confidence: f64, project: Option<&str>, tags: Vec<&str>) {
        let now = Utc::now();
        store.conn.execute(
            "INSERT INTO insights (id, title, content, confidence, project, tags, updated_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, title, content, confidence, project, serde_json::to_string(&tags).unwrap(), now.to_rfc3339(), now.to_rfc3339()],
        ).unwrap();
    }

    #[test]
    fn test_list_returns_insights() {
        let store = test_store();
        insert_test_insight(&store, "i-1", "Auth insight", "JWT is best", 0.9, None, vec!["auth"]);
        let insights = store.list(None, 0.0, 10).unwrap();
        assert_eq!(insights.len(), 1);
    }

    #[test]
    fn test_list_filters_by_project() {
        let store = test_store();
        insert_test_insight(&store, "i-1", "A", "Content", 0.8, Some("proj-a"), vec![]);
        insert_test_insight(&store, "i-2", "B", "Content", 0.8, Some("proj-b"), vec![]);
        let insights = store.list(Some("proj-a"), 0.0, 10).unwrap();
        assert_eq!(insights.len(), 1);
    }

    #[test]
    fn test_search_finds_by_query() {
        let store = test_store();
        insert_test_insight(&store, "i-1", "JWT auth", "Use JWT for auth", 0.8, None, vec!["auth"]);
        insert_test_insight(&store, "i-2", "CSS fix", "Flexbox layout", 0.8, None, vec!["css"]);
        let results = store.search("JWT auth", None, 0.1, 10).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_decay_sweep_reduces_confidence() {
        let store = test_store();
        let now = Utc::now();
        let old = now - chrono::Duration::weeks(4);
        store.conn.execute(
            "INSERT INTO insights (id, title, content, confidence, decay_rate, last_reinforced_at, updated_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params!["i-1", "Old insight", "Content", 0.8, 0.05, old.to_rfc3339(), old.to_rfc3339(), old.to_rfc3339()],
        ).unwrap();

        let (decayed, soft_deleted, total) = store.decay_sweep().unwrap();
        assert!(total >= 1);
        assert!(decayed > 0 || soft_deleted > 0);
    }

    #[test]
    fn test_list_filters_by_min_confidence() {
        let store = test_store();
        insert_test_insight(&store, "i-1", "High", "Content", 0.9, None, vec![]);
        insert_test_insight(&store, "i-2", "Low", "Content", 0.2, None, vec![]);
        let insights = store.list(None, 0.5, 10).unwrap();
        assert_eq!(insights.len(), 1);
    }

    #[test]
    fn test_search_case_insensitive() {
        let store = test_store();
        insert_test_insight(&store, "i-1", "JWT Auth", "Use JWT", 0.8, None, vec![]);
        let results = store.search("jwt", None, 0.1, 10).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_search_empty_query() {
        let store = test_store();
        insert_test_insight(&store, "i-1", "Test", "Content", 0.8, None, vec![]);
        let results = store.search("", None, 0.1, 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_list_empty() {
        let store = test_store();
        let insights = store.list(None, 0.0, 10).unwrap();
        assert!(insights.is_empty());
    }
}
