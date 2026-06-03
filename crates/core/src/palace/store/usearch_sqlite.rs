use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::palace::{Drawer, DrawerId, PalaceStore, SearchHit, SearchScope, StoreTier};

pub struct UsearchSqliteStore {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    index: usearch::Index,
    db: rusqlite::Connection,
}

impl Inner {
    fn open(path: &Path, dim: usize) -> anyhow::Result<Self> {
        std::fs::create_dir_all(path)?;
        let index_path = path.join("index.usearch");
        let db_path = path.join("drawers.sqlite");

        let index = if index_path.exists() {
            usearch::Index::restore(index_path.to_string_lossy().as_ref())
                .map_err(|e| anyhow::anyhow!("usearch restore: {e}"))?
        } else {
            let opts = usearch::IndexOptions {
                dimensions: dim,
                metric: usearch::MetricKind::Cos,
                quantization: usearch::ScalarKind::F32,
                connectivity: 0,
                expansion_add: 0,
                expansion_search: 0,
                multi: false,
            };
            let idx =
                usearch::Index::new(&opts).map_err(|e| anyhow::anyhow!("usearch new: {e}"))?;
            let _ = idx.reserve(10_000).is_err();
            idx
        };

        let db = rusqlite::Connection::open(&db_path)?;
        db.execute(
            "CREATE TABLE IF NOT EXISTS drawers (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                kind TEXT NOT NULL,
                tier TEXT NOT NULL,
                wing TEXT,
                room TEXT,
                metadata TEXT
            )",
            [],
        )?;

        Ok(Self { index, db })
    }

    fn upsert_drawers(&self, drawers: Vec<Drawer>) -> anyhow::Result<()> {
        let mut stmt = self.db.prepare(
            "INSERT OR REPLACE INTO drawers (id, content, kind, tier, wing, room, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        // mp-25: SELECT existing row's metadata first so we can preserve
        // `created_at` across re-upserts. The schema doesn't have
        // dedicated created_at/updated_at columns, so we piggyback on
        // the `metadata` JSON column (re-parsed by `migrate_metadata`
        // on the next read).
        let mut lookup = self
            .db
            .prepare("SELECT metadata FROM drawers WHERE id = ?1")?;
        for mut drawer in drawers {
            let id = drawer.id.clone().map(|d| d.0).unwrap_or_default();
            // Touch `updated_at` to mark this write. The serialised
            // drawer that the caller passed in may carry a stale
            // timestamp from when it was read out of the store
            // minutes ago.
            drawer.touch();
            if let Some(prev_meta_str) = lookup
                .query_row(rusqlite::params![id], |r| r.get::<_, String>(0))
                .ok()
            {
                if let Ok(prev_meta) = serde_json::from_str::<
                    std::collections::HashMap<String, serde_json::Value>,
                >(&prev_meta_str)
                {
                    if let Some(prev_created) = prev_meta
                        .get("created_at")
                        .and_then(|v| v.as_str())
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|dt| dt.with_timezone(&chrono::Utc))
                    {
                        // Preserve the original creation time, even if
                        // the caller constructed a fresh Drawer that
                        // says "now".
                        drawer.created_at = prev_created;
                    }
                }
            }
            // Write the typed fields back into the metadata map so
            // they survive the round-trip through SQLite.
            drawer.metadata.insert(
                "created_at".to_string(),
                serde_json::Value::String(drawer.created_at.to_rfc3339()),
            );
            drawer.metadata.insert(
                "updated_at".to_string(),
                serde_json::Value::String(drawer.updated_at.to_rfc3339()),
            );
            let kind = serde_json::to_string(&drawer.kind).unwrap_or_default();
            let tier = serde_json::to_string(&drawer.tier).unwrap_or_default();
            let metadata = serde_json::to_string(&drawer.metadata).unwrap_or_default();
            stmt.execute(rusqlite::params![
                id,
                drawer.content,
                kind,
                tier,
                drawer.wing,
                drawer.room,
                metadata
            ])?;
        }
        Ok(())
    }

    fn search_index(&self, query: &[f32], limit: usize) -> anyhow::Result<Vec<(String, f32)>> {
        let results = self.index.search(query, limit)?;
        let mut out = Vec::with_capacity(results.keys.len());
        for i in 0..results.keys.len() {
            out.push((results.keys[i].to_string(), results.distances[i]));
        }
        Ok(out)
    }

    fn get_drawer_by_id(&self, id: &str) -> anyhow::Result<Option<Drawer>> {
        let mut stmt = self.db.prepare(
            "SELECT id, content, kind, tier, wing, room, metadata FROM drawers WHERE id = ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![id])?;
        if let Some(row) = rows.next()? {
            let id_str: String = row.get(0)?;
            let kind_str: String = row.get(2)?;
            let tier_str: String = row.get(3)?;
            let metadata_str: String = row.get(6)?;
            // mp-migration 24/8: auto-migrate legacy drawers on
            // every read so callers see the v1 (typed-field) shape
            // even if the data was written by a pre-PR #7 Palace.
            let mut drawer = Drawer {
                id: Some(DrawerId(id_str)),
                content: row.get(1)?,
                kind: serde_json::from_str(&kind_str).unwrap_or_default(),
                tier: serde_json::from_str(&tier_str).unwrap_or_default(),
                wing: row.get(4)?,
                room: row.get(5)?,
                metadata: serde_json::from_str(&metadata_str).unwrap_or_default(),
                derived_from: vec![],
                tags: Vec::new(),
                trust: None,
                access_count: 0,
                last_accessed: None,
                reinforcements: Vec::new(),
                superseded_by: None,
                active: true,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            };
            drawer.migrate_metadata();
            Ok(Some(drawer))
        } else {
            Ok(None)
        }
    }

    fn count_drawers(&self) -> anyhow::Result<usize> {
        let n: i64 = self
            .db
            .query_row("SELECT COUNT(*) FROM drawers", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    fn all_drawers(&self, limit: Option<usize>) -> anyhow::Result<Vec<Drawer>> {
        let limit = limit.unwrap_or(1000) as i64;
        let mut stmt = self.db.prepare(
            "SELECT id, content, kind, tier, wing, room, metadata FROM drawers LIMIT ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![limit])?;
        let mut drawers = Vec::new();
        while let Some(row) = rows.next()? {
            let id_str: String = row.get(0)?;
            let kind_str: String = row.get(2)?;
            let tier_str: String = row.get(3)?;
            let metadata_str: String = row.get(6)?;
            // mp-migration 24/8: auto-migrate legacy drawers on
            // every read (see comment in get_drawer_by_id above).
            let mut drawer = Drawer {
                id: Some(DrawerId(id_str)),
                content: row.get(1)?,
                kind: serde_json::from_str(&kind_str).unwrap_or_default(),
                tier: serde_json::from_str(&tier_str).unwrap_or_default(),
                wing: row.get(4)?,
                room: row.get(5)?,
                metadata: serde_json::from_str(&metadata_str).unwrap_or_default(),
                derived_from: vec![],
                tags: Vec::new(),
                trust: None,
                access_count: 0,
                last_accessed: None,
                reinforcements: Vec::new(),
                superseded_by: None,
                active: true,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            };
            drawer.migrate_metadata();
            drawers.push(drawer);
        }
        Ok(drawers)
    }
}

impl UsearchSqliteStore {
    pub async fn open(path: &Path, dim: usize) -> anyhow::Result<Self> {
        let path_buf = path.to_path_buf();
        let inner = tokio::task::spawn_blocking(move || Inner::open(&path_buf, dim))
            .await
            .map_err(|e| anyhow::anyhow!("Join error: {e}"))??;
        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
        })
    }
}

#[async_trait]
impl PalaceStore for UsearchSqliteStore {
    async fn upsert(&self, drawers: Vec<Drawer>) -> anyhow::Result<()> {
        let inner = self.inner.lock().await;
        inner.upsert_drawers(drawers)?;
        Ok(())
    }

    async fn delete(&self, _ids: &[DrawerId]) -> anyhow::Result<usize> {
        Ok(0)
    }

    async fn search(
        &self,
        query: &[f32],
        scope: &SearchScope,
        limit: usize,
    ) -> anyhow::Result<Vec<SearchHit>> {
        let inner = self.inner.lock().await;
        let results = inner.search_index(query, limit)?;
        let mut hits = Vec::with_capacity(results.len());
        for (id, dist) in results {
            if let Some(d) = inner.get_drawer_by_id(&id)? {
                let source_file = d
                    .metadata
                    .get("source_file")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                hits.push(SearchHit {
                    text: d.content,
                    wing: d.wing.or_else(|| scope.wing.clone()),
                    room: d.room.or_else(|| scope.room.clone()),
                    source_file,
                    similarity: dist as f64,
                    bm25_score: None,
                    combined_score: None,
                });
            }
        }
        Ok(hits)
    }

    async fn count(&self, _scope: &SearchScope) -> anyhow::Result<usize> {
        let inner = self.inner.lock().await;
        inner.count_drawers()
    }

    async fn flush(&self) -> anyhow::Result<()> {
        let inner = self.inner.lock().await;
        inner
            .index
            .save("index.usearch")
            .map_err(|e| anyhow::anyhow!("usearch save: {e}"))?;
        Ok(())
    }

    fn tier(&self) -> StoreTier {
        StoreTier::Usearch
    }

    async fn get_drawers(
        &self,
        _scope: Option<&SearchScope>,
        limit: Option<usize>,
    ) -> anyhow::Result<Vec<Drawer>> {
        let inner = self.inner.lock().await;
        inner.all_drawers(limit)
    }
}
