use crate::types::Lesson;
use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection};
use std::collections::HashMap;

pub struct LessonStore {
    conn: Connection,
}

impl LessonStore {
    pub fn new(conn: Connection) -> Result<Self> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS lessons (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                context TEXT,
                retention REAL NOT NULL DEFAULT 0.5,
                confidence REAL NOT NULL DEFAULT 0.5,
                project TEXT,
                source TEXT,
                source_ids TEXT NOT NULL DEFAULT '[]',
                tags TEXT NOT NULL DEFAULT '[]',
                reinforcement_count INTEGER NOT NULL DEFAULT 0,
                decay_rate REAL NOT NULL DEFAULT 0.05,
                last_reinforced TEXT,
                last_decayed_at TEXT,
                updated_at TEXT NOT NULL,
                created_at TEXT NOT NULL,
                deleted INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )?;
        Ok(Self { conn })
    }

    pub fn save(
        &self,
        content: &str,
        context: Option<&str>,
        confidence: f64,
        project: Option<&str>,
        tags: Vec<String>,
        source: Option<&str>,
        source_ids: Vec<String>,
    ) -> Result<Lesson> {
        let fp = fingerprint(content);
        let existing = self.get(&fp)?;

        if let Some(mut lesson) = existing {
            if !lesson.deleted {
                reinforce_lesson(&mut lesson);
                if let Some(ctx) = context {
                    if lesson.context.is_none() {
                        lesson.context = Some(ctx.to_string());
                    }
                }
                self.update(&lesson)?;
                return Ok(lesson);
            }
        }

        let now = Utc::now();
        let lesson = Lesson {
            id: fp,
            content: content.trim().to_string(),
            context: context.map(String::from),
            retention: 0.5,
            confidence: confidence.clamp(0.0, 1.0),
            project: project.map(String::from),
            source: source.map(String::from),
            source_ids,
            tags,
            last_reinforced: None,
            reinforcement_count: 0,
            decay_rate: 0.05,
            last_decayed_at: None,
            updated_at: now,
            deleted: false,
            created_at: now,
        };

        self.insert(&lesson)?;
        Ok(lesson)
    }

    pub fn recall(
        &self,
        query: &str,
        project: Option<&str>,
        min_confidence: f64,
        limit: usize,
    ) -> Result<Vec<(Lesson, f64)>> {
        let lessons = self.list(project, min_confidence, 1000)?;
        let terms: Vec<&str> = query.split_whitespace().filter(|t| t.len() > 1).collect();

        let mut scored: Vec<(Lesson, f64)> = lessons
            .into_iter()
            .filter_map(|l| {
                let text = format!(
                    "{} {} {}",
                    l.content,
                    l.context.as_deref().unwrap_or(""),
                    l.tags.join(" ")
                )
                .to_lowercase();
                let match_count = terms.iter().filter(|t| text.contains(*t)).count();
                if match_count == 0 {
                    return None;
                }
                let relevance = match_count as f64 / terms.len() as f64;
                let days_since = l
                    .last_reinforced
                    .map(|t| (Utc::now() - t).num_days() as f64)
                    .unwrap_or(0.0);
                let recency_boost = 1.0 / (1.0 + days_since * 0.01);
                let score = l.confidence * relevance * recency_boost;
                Some((l, score))
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        Ok(scored)
    }

    pub fn list(
        &self,
        project: Option<&str>,
        min_confidence: f64,
        limit: usize,
    ) -> Result<Vec<Lesson>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content, context, retention, confidence, project, source,
                    source_ids, tags, reinforcement_count, decay_rate,
                    last_reinforced, last_decayed_at, updated_at, created_at, deleted
             FROM lessons WHERE deleted = 0 AND confidence >= ?1
             ORDER BY confidence DESC LIMIT ?2",
        )?;

        let lessons: Vec<Lesson> = stmt
            .query_map(params![min_confidence, limit], |row| {
                let source_ids: String = row.get(7)?;
                let tags: String = row.get(8)?;
                Ok(Lesson {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    context: row.get(2)?,
                    retention: row.get(3)?,
                    confidence: row.get(4)?,
                    project: row.get(5)?,
                    source: row.get(6)?,
                    source_ids: serde_json::from_str(&source_ids).unwrap_or_default(),
                    tags: serde_json::from_str(&tags).unwrap_or_default(),
                    reinforcement_count: row.get(9)?,
                    decay_rate: row.get(10)?,
                    last_reinforced: row.get::<_, Option<String>>(11)?.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok()).map(|dt| dt.with_timezone(&Utc)),
                    last_decayed_at: row.get::<_, Option<String>>(12)?.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok()).map(|dt| dt.with_timezone(&Utc)),
                    updated_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(13)?).unwrap().with_timezone(&Utc),
                    created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(14)?).unwrap().with_timezone(&Utc),
                    deleted: row.get::<_, i32>(15)? != 0,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        if let Some(proj) = project {
            Ok(lessons.into_iter().filter(|l| l.project.as_deref() == Some(proj)).take(limit).collect())
        } else {
            Ok(lessons)
        }
    }

    pub fn strengthen(&self, id: &str) -> Result<Lesson> {
        let mut lesson = self.get(id)?.ok_or_else(|| anyhow::anyhow!("Lesson not found"))?;
        if lesson.deleted {
            return Err(anyhow::anyhow!("Lesson is deleted"));
        }
        reinforce_lesson(&mut lesson);
        self.update(&lesson)?;
        Ok(lesson)
    }

    pub fn decay_sweep(&self) -> Result<(usize, usize, usize)> {
        let lessons = self.list(None, 0.0, 10000)?;
        let now = Utc::now();
        let mut decayed = 0;
        let mut soft_deleted = 0;
        let total = lessons.len();

        for lesson in lessons {
            let baseline = lesson
                .last_decayed_at
                .or(lesson.last_reinforced)
                .unwrap_or(lesson.created_at);
            let weeks = (now - baseline).num_weeks() as f64;
            if weeks < 1.0 {
                continue;
            }

            let decay = lesson.decay_rate * weeks;
            let new_confidence = (lesson.confidence - decay).max(0.05);

            if (new_confidence - lesson.confidence).abs() > 0.001 {
                let mut updated_lesson = lesson;
                updated_lesson.confidence = new_confidence;
                updated_lesson.last_decayed_at = Some(now);
                updated_lesson.updated_at = now;

                if updated_lesson.confidence <= 0.1 && updated_lesson.reinforcement_count == 0 {
                    updated_lesson.deleted = true;
                    soft_deleted += 1;
                } else {
                    decayed += 1;
                }

                self.update(&updated_lesson)?;
            }
        }

        Ok((decayed, soft_deleted, total))
    }

    fn get(&self, id: &str) -> Result<Option<Lesson>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content, context, retention, confidence, project, source,
                    source_ids, tags, reinforcement_count, decay_rate,
                    last_reinforced, last_decayed_at, updated_at, created_at, deleted
             FROM lessons WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            let source_ids: String = row.get(7)?;
            let tags: String = row.get(8)?;
            Ok(Some(Lesson {
                id: row.get(0)?,
                content: row.get(1)?,
                context: row.get(2)?,
                retention: row.get(3)?,
                confidence: row.get(4)?,
                project: row.get(5)?,
                source: row.get(6)?,
                source_ids: serde_json::from_str(&source_ids).unwrap_or_default(),
                tags: serde_json::from_str(&tags).unwrap_or_default(),
                reinforcement_count: row.get(9)?,
                decay_rate: row.get(10)?,
                last_reinforced: row.get::<_, Option<String>>(11)?.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok()).map(|dt| dt.with_timezone(&Utc)),
                last_decayed_at: row.get::<_, Option<String>>(12)?.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok()).map(|dt| dt.with_timezone(&Utc)),
                updated_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(13)?).unwrap().with_timezone(&Utc),
                created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>(14)?).unwrap().with_timezone(&Utc),
                deleted: row.get::<_, i32>(15)? != 0,
            }))
        } else {
            Ok(None)
        }
    }

    fn insert(&self, lesson: &Lesson) -> Result<()> {
        self.conn.execute(
            "INSERT INTO lessons (id, content, context, retention, confidence, project, source,
                                  source_ids, tags, reinforcement_count, decay_rate,
                                  last_reinforced, last_decayed_at, updated_at, created_at, deleted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                lesson.id, lesson.content, lesson.context, lesson.retention, lesson.confidence,
                lesson.project, lesson.source, serde_json::to_string(&lesson.source_ids)?,
                serde_json::to_string(&lesson.tags)?, lesson.reinforcement_count, lesson.decay_rate,
                lesson.last_reinforced.map(|d| d.to_rfc3339()),
                lesson.last_decayed_at.map(|d| d.to_rfc3339()),
                lesson.updated_at.to_rfc3339(), lesson.created_at.to_rfc3339(),
                lesson.deleted as i32
            ],
        )?;
        Ok(())
    }

    fn update(&self, lesson: &Lesson) -> Result<()> {
        self.conn.execute(
            "UPDATE lessons SET content=?2, context=?3, retention=?4, confidence=?5,
                                project=?6, source=?7, source_ids=?8, tags=?9,
                                reinforcement_count=?10, decay_rate=?11,
                                last_reinforced=?12, last_decayed_at=?13,
                                updated_at=?14, deleted=?15 WHERE id=?1",
            params![
                lesson.id, lesson.content, lesson.context, lesson.retention, lesson.confidence,
                lesson.project, lesson.source, serde_json::to_string(&lesson.source_ids)?,
                serde_json::to_string(&lesson.tags)?, lesson.reinforcement_count, lesson.decay_rate,
                lesson.last_reinforced.map(|d| d.to_rfc3339()),
                lesson.last_decayed_at.map(|d| d.to_rfc3339()),
                lesson.updated_at.to_rfc3339(), lesson.deleted as i32
            ],
        )?;
        Ok(())
    }
}

fn reinforce_lesson(lesson: &mut Lesson) {
    let now = Utc::now();
    lesson.reinforcement_count += 1;
    lesson.confidence = (lesson.confidence + 0.1 * (1.0 - lesson.confidence)).min(1.0);
    lesson.last_reinforced = Some(now);
    lesson.updated_at = now;
}

fn fingerprint(content: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    content.trim().to_lowercase().hash(&mut hasher);
    format!("lsn-{:x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> LessonStore {
        let conn = Connection::open_in_memory().unwrap();
        LessonStore::new(conn).unwrap()
    }

    #[test]
    fn test_save_creates_lesson() {
        let store = test_store();
        let lesson = store.save("Always use Rust", None, 0.8, None, vec![], Some("manual"), vec![]).unwrap();
        assert!(lesson.id.starts_with("lsn-"));
        assert_eq!(lesson.content, "Always use Rust");
        assert!((lesson.confidence - 0.8).abs() < 0.01);
    }

    #[test]
    fn test_save_deduplicates_and_strengthens() {
        let store = test_store();
        store.save("Always use Rust", None, 0.5, None, vec![], None, vec![]).unwrap();
        let lesson2 = store.save("always use rust", None, 0.5, None, vec![], None, vec![]).unwrap();
        assert_eq!(lesson2.reinforcement_count, 1);
        assert!(lesson2.confidence > 0.5);
    }

    #[test]
    fn test_recall_scores_by_relevance() {
        let store = test_store();
        store.save("JWT auth middleware", None, 0.8, None, vec!["auth".to_string()], None, vec![]).unwrap();
        store.save("CSS layout fix", None, 0.8, None, vec!["css".to_string()], None, vec![]).unwrap();
        let results = store.recall("JWT auth", None, 0.1, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].0.content.contains("JWT"));
    }

    #[test]
    fn test_list_filters_by_project() {
        let store = test_store();
        store.save("Lesson A", None, 0.8, Some("proj-a"), vec![], None, vec![]).unwrap();
        store.save("Lesson B", None, 0.8, Some("proj-b"), vec![], None, vec![]).unwrap();
        let lessons = store.list(Some("proj-a"), 0.0, 10).unwrap();
        assert_eq!(lessons.len(), 1);
        assert_eq!(lessons[0].content, "Lesson A");
    }

    #[test]
    fn test_strengthen_increases_confidence() {
        let store = test_store();
        let lesson = store.save("Test lesson", None, 0.5, None, vec![], None, vec![]).unwrap();
        let strengthened = store.strengthen(&lesson.id).unwrap();
        assert!(strengthened.confidence > 0.5);
        assert_eq!(strengthened.reinforcement_count, 1);
    }

    #[test]
    fn test_decay_sweep_reduces_confidence() {
        let store = test_store();
        let mut lesson = store.save("Old lesson", None, 0.8, None, vec![], None, vec![]).unwrap();
        lesson.created_at = Utc::now() - chrono::Duration::weeks(4);
        lesson.last_reinforced = Some(Utc::now() - chrono::Duration::weeks(4));
        store.update(&lesson).unwrap();

        let (decayed, soft_deleted, total) = store.decay_sweep().unwrap();
        assert!(total >= 1);
        assert!(decayed > 0 || soft_deleted > 0);
    }

    #[test]
    fn test_list_filters_by_min_confidence() {
        let store = test_store();
        store.save("High conf", None, 0.9, None, vec![], None, vec![]).unwrap();
        store.save("Low conf", None, 0.2, None, vec![], None, vec![]).unwrap();
        let lessons = store.list(None, 0.5, 10).unwrap();
        assert_eq!(lessons.len(), 1);
        assert_eq!(lessons[0].content, "High conf");
    }

    #[test]
    fn test_recall_empty_query() {
        let store = test_store();
        store.save("Test lesson", None, 0.8, None, vec![], None, vec![]).unwrap();
        let results = store.recall("", None, 0.1, 10).unwrap();
        assert!(results.is_empty());
    }
}
