//! SQLite-based drawer storage with FTS5 full-text search.
//!
//! Replaces the legacy JSON file (`mempalace_drawers.json`) with an
//! incremental, indexed SQLite database. The schema stores each drawer
//! as a row and maintains an FTS5 virtual table for fast full-text
//! search across `content`, `wing`, and `room` columns.
//!
//! # Migration
//!
//! [`DrawerStore::migrate_from_json`] reads an existing JSON map and
//! bulk-inserts all entries into SQLite. After migration the JSON file
//! can be deleted (the store never reads it except during migration).
//!
//! # Backward compatibility
//!
//! [`PalaceDb`] still holds a `documents: HashMap<String, DocumentEntry>`
//! for the many existing code paths that iterate in-memory. When a
//! [`DrawerStore`] is present, writes go to SQLite *and* the HashMap;
//! reads work from the HashMap (fast). The `save()` method becomes a
//! no-op because SQLite writes are incremental.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde_json::Value;
use tracing::info;

use crate::palace_db::DocumentEntry;

/// SQLite-backed drawer store with FTS5 search.
pub struct DrawerStore {
    conn: Mutex<Connection>,
}

impl DrawerStore {
    /// Open (or create) the drawers SQLite database at `palace_path/drawers.db`.
    ///
    /// Creates the schema if it does not exist, including FTS5 virtual
    /// tables and triggers. WAL journal mode is enabled for better
    /// concurrent-read performance.
    pub fn open(palace_path: &Path) -> Result<Self> {
        let db_path = palace_path.join("drawers.db");
        let conn = Connection::open(&db_path)
            .with_context(|| format!("failed to open drawer store at {}", db_path.display()))?;

        // Enable WAL mode for better concurrent-read performance
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;

        // Create schema
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS drawers (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                metadata TEXT NOT NULL DEFAULT '{}',
                wing TEXT NOT NULL DEFAULT '',
                room TEXT NOT NULL DEFAULT '',
                source_file TEXT,
                filed_at TEXT NOT NULL DEFAULT (datetime('now')),
                source_mtime REAL
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS drawers_fts USING fts5(
                content, wing, room,
                content=drawers,
                content_rowid=rowid,
                tokenize='porter unicode61'
            );

            CREATE TRIGGER IF NOT EXISTS drawers_ai AFTER INSERT ON drawers BEGIN
                INSERT INTO drawers_fts(rowid, content, wing, room)
                VALUES (new.rowid, new.content, new.wing, new.room);
            END;

            CREATE TRIGGER IF NOT EXISTS drawers_ad AFTER DELETE ON drawers BEGIN
                INSERT INTO drawers_fts(drawers_fts, rowid, content, wing, room)
                VALUES('delete', old.rowid, old.content, old.wing, old.room);
            END;

            CREATE TRIGGER IF NOT EXISTS drawers_au AFTER UPDATE ON drawers BEGIN
                INSERT INTO drawers_fts(drawers_fts, rowid, content, wing, room)
                VALUES('delete', old.rowid, old.content, old.wing, old.room);
                INSERT INTO drawers_fts(rowid, content, wing, room)
                VALUES (new.rowid, new.content, new.wing, new.room);
            END;",
        )?;

        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Return the number of drawers in the store.
    pub fn len(&self) -> usize {
        self.conn
            .lock()
            .expect("conn")
            .query_row("SELECT COUNT(*) FROM drawers", [], |row| row.get::<_, i64>(0))
            .unwrap_or(0) as usize
    }

    /// Returns true if the store has no drawers.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Load all drawers into a HashMap compatible with `PalaceDb::documents`.
    ///
    /// Used during [`PalaceDb::open`] to populate the in-memory cache.
    /// Returns `id → DocumentEntry` mappings suitable for direct use.
    pub fn load_all_to_hashmap(&self) -> Result<HashMap<String, DocumentEntry>> {
        let guard = self.conn.lock().expect("conn");
        let mut stmt = guard
            .prepare("SELECT id, content, metadata, wing, room FROM drawers")?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let content: String = row.get(1)?;
            let metadata_str: String = row.get(2)?;
            let wing: String = row.get(3)?;
            let room: String = row.get(4)?;

            // Parse metadata JSON, add wing/room
            let mut metadata: HashMap<String, Value> =
                serde_json::from_str(&metadata_str).unwrap_or_default();
            if !wing.is_empty() {
                metadata.insert("wing".to_string(), Value::String(wing));
            }
            if !room.is_empty() {
                metadata.insert("room".to_string(), Value::String(room));
            }

            Ok((id, DocumentEntry { content, metadata }))
        })?;

        let mut documents = HashMap::new();
        for row in rows {
            let (id, entry) = row?;
            documents.insert(id, entry);
        }
        Ok(documents)
    }

    /// Get all drawers, optionally filtered by wing and/or room, with a limit.
    ///
    /// Returns `Vec<(id, content, metadata)>` matching the filter criteria.
    pub fn get_all(
        &self,
        wing: Option<&str>,
        room: Option<&str>,
        limit: usize,
    ) -> Result<Vec<(String, String, HashMap<String, Value>)>> {
        let mut sql = String::from(
            "SELECT id, content, metadata, wing, room FROM drawers WHERE 1=1",
        );
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(w) = wing {
            sql.push_str(" AND wing = ?");
            param_values.push(Box::new(w.to_string()));
        }
        if let Some(r) = room {
            sql.push_str(" AND room = ?");
            param_values.push(Box::new(r.to_string()));
        }
        sql.push_str(" ORDER BY filed_at DESC");
        sql.push_str(&format!(" LIMIT {}", limit));

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();

        let guard = self.conn.lock().expect("conn");
        let mut stmt = guard.prepare(&sql)?;
        let rows = stmt.query_map(params_refs.as_slice(), |row| {
            let id: String = row.get(0)?;
            let content: String = row.get(1)?;
            let metadata_str: String = row.get(2)?;
            let wing: String = row.get(3)?;
            let room: String = row.get(4)?;

            let mut metadata: HashMap<String, Value> =
                serde_json::from_str(&metadata_str).unwrap_or_default();
            if !wing.is_empty() {
                metadata.insert("wing".to_string(), Value::String(wing));
            }
            if !room.is_empty() {
                metadata.insert("room".to_string(), Value::String(room));
            }

            Ok((id, content, metadata))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get a single drawer by ID.
    ///
    /// Returns `Some((content, metadata))` if found, `None` otherwise.
    pub fn get_by_id(&self, id: &str) -> Result<Option<(String, HashMap<String, Value>)>> {
        let guard = self.conn.lock().expect("conn");
        let mut stmt = guard.prepare(
            "SELECT content, metadata, wing, room FROM drawers WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            let content: String = row.get(0)?;
            let metadata_str: String = row.get(1)?;
            let wing: String = row.get(2)?;
            let room: String = row.get(3)?;

            let mut metadata: HashMap<String, Value> =
                serde_json::from_str(&metadata_str).unwrap_or_default();
            if !wing.is_empty() {
                metadata.insert("wing".to_string(), Value::String(wing));
            }
            if !room.is_empty() {
                metadata.insert("room".to_string(), Value::String(room));
            }

            Ok(Some((content, metadata)))
        } else {
            Ok(None)
        }
    }

    /// Search drawers using FTS5 MATCH.
    ///
    /// Returns `Vec<(id, content, score)>` ordered by descending BM25 score.
    pub fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(String, String, f64)>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        // Build an FTS5 query from the user's terms.
        // FTS5 supports simple term queries; we join terms with AND for
        // precision. Escape special characters and convert to term queries.
        let fts_query = build_fts_query(query);

        let sql = format!(
            "SELECT d.id, d.content, bm25(drawers_fts, 0.0, 0.0, 1.0, 1.0) AS score
             FROM drawers_fts
             JOIN drawers ON drawers.rowid = drawers_fts.rowid
             WHERE drawers_fts MATCH ?1
             ORDER BY score
             LIMIT ?2"
        );

        let guard = self.conn.lock().expect("conn");
        let mut stmt = guard.prepare(&sql)?;
        let rows = stmt.query_map(params![fts_query, limit as i64], |row| {
            let id: String = row.get(0)?;
            let content: String = row.get(1)?;
            let score: f64 = row.get(2)?;
            Ok((id, content, score))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Search drawers using FTS5 MATCH and return results with metadata.
    ///
    /// Returns `Vec<(id, content, metadata, score)>` ordered by BM25 score.
    pub fn search_with_metadata(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(String, String, HashMap<String, Value>, f64)>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let fts_query = build_fts_query(query);

        let sql = format!(
            "SELECT d.id, d.content, d.metadata, d.wing, d.room,
                    bm25(drawers_fts, 0.0, 0.0, 1.0, 1.0) AS score
             FROM drawers_fts
             JOIN drawers ON drawers.rowid = drawers_fts.rowid
             WHERE drawers_fts MATCH ?1
             ORDER BY score
             LIMIT ?2"
        );

        let guard = self.conn.lock().expect("conn");
        let mut stmt = guard.prepare(&sql)?;
        let rows = stmt.query_map(params![fts_query, limit as i64], |row| {
            let id: String = row.get(0)?;
            let content: String = row.get(1)?;
            let metadata_str: String = row.get(2)?;
            let wing: String = row.get(3)?;
            let room: String = row.get(4)?;
            let score: f64 = row.get(5)?;

            let mut metadata: HashMap<String, Value> =
                serde_json::from_str(&metadata_str).unwrap_or_default();
            if !wing.is_empty() {
                metadata.insert("wing".to_string(), Value::String(wing));
            }
            if !room.is_empty() {
                metadata.insert("room".to_string(), Value::String(room));
            }

            Ok((id, content, metadata, score))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Insert a single drawer.
    ///
    /// Extracts `wing`, `room`, and `source_file` from metadata if present.
    pub fn insert(
        &self,
        id: &str,
        content: &str,
        metadata: &HashMap<String, Value>,
        wing: &str,
        room: &str,
        source_file: Option<&str>,
        source_mtime: Option<f64>,
    ) -> Result<()> {
        let metadata_json = serde_json::to_string(metadata)?;

        // Strip wing/room from metadata JSON to avoid duplication
        // (they're stored as separate columns)
        let mut clean_meta = metadata.clone();
        clean_meta.remove("wing");
        clean_meta.remove("room");
        let clean_meta_json = serde_json::to_string(&clean_meta)?;

        let guard = self.conn.lock().expect("conn");
        guard.execute("INSERT OR REPLACE INTO drawers (id, content, metadata, wing, room, source_file, source_mtime)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                content,
                clean_meta_json,
                wing,
                room,
                source_file,
                source_mtime,
            ],
        )?;
        Ok(())
    }

    /// Batch-insert multiple drawers in a single transaction.
    pub fn insert_batch(
        &self,
        items: &[(
            &str,           // id
            &str,           // content
            &HashMap<String, Value>, // metadata
            &str,           // wing
            &str,           // room
            Option<&str>,   // source_file
            Option<f64>,    // source_mtime
        )],
    ) -> Result<()> {
        let guard_tx = self.conn.lock().expect("conn");
        let tx = guard_tx.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO drawers (id, content, metadata, wing, room, source_file, source_mtime)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;

            for &(id, content, metadata, wing, room, source_file, source_mtime) in items {
                let mut clean_meta = metadata.clone();
                clean_meta.remove("wing");
                clean_meta.remove("room");
                let clean_meta_json = serde_json::to_string(&clean_meta)?;

                stmt.execute(params![
                    id,
                    content,
                    clean_meta_json,
                    wing,
                    room,
                    source_file,
                    source_mtime,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Delete a drawer by ID.
    pub fn delete(&self, id: &str) -> Result<bool> {
        let rows = self
            .conn
            .lock()
            .expect("conn")
            .execute("DELETE FROM drawers WHERE id = ?1", params![id])?;
        Ok(rows > 0)
    }

    /// Delete all drawers that have a given source_file.
    pub fn delete_by_source(&self, source_file: &str) -> Result<usize> {
        let guard = self.conn.lock().expect("conn");
        let rows = guard.execute(
            "DELETE FROM drawers WHERE source_file = ?1",
            params![source_file],
        )?;
        Ok(rows)
    }

    /// Streaming export: iterate all drawers grouped by source_file,
    /// writing one output file per source_file.
    ///
    /// `format` determines the output format. Currently supports
    /// `"basic-memory"` (Obsidian-compatible Markdown) and `"markdown"`.
    pub fn export_stream(
        &self,
        output_dir: &Path,
        format: &str,
    ) -> Result<()> {
        let guard = self.conn.lock().expect("conn");
        let mut stmt = guard.prepare(
            "SELECT id, content, metadata, wing, room, source_file, filed_at
             FROM drawers ORDER BY source_file, filed_at",
        )?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let content: String = row.get(1)?;
            let metadata_str: String = row.get(2)?;
            let wing: String = row.get(3)?;
            let room: String = row.get(4)?;
            let source_file: Option<String> = row.get(5)?;
            let filed_at: String = row.get(6)?;
            Ok((
                id,
                content,
                metadata_str,
                wing,
                room,
                source_file,
                filed_at,
            ))
        })?;

        let mut current_source: Option<String> = None;
        let mut current_file: Option<std::fs::File> = None;

        for row in rows {
            let (id, content, metadata_str, wing, room, source_file, filed_at) = row?;

            let source = source_file.as_deref().unwrap_or("unknown");

            if current_source.as_deref() != Some(source) {
                // Close previous file
                if let Some(mut f) = current_file.take() {
                    use std::io::Write;
                    let _ = writeln!(f);
                }

                // Open new file for this source
                let safe_name = source.replace('/', "_").replace('\\', "_");
                let out_path = output_dir.join(format!("{}.md", safe_name));
                if let Some(parent) = out_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let file = std::fs::File::create(&out_path)
                    .with_context(|| format!("creating export file {}", out_path.display()))?;
                current_source = Some(source.to_string());
                current_file = Some(file);
            }

            if let Some(ref mut f) = current_file {
                use std::io::Write;
                match format {
                    "basic-memory" | "markdown" => {
                        writeln!(f, "## {}\n", id)?;
                        writeln!(f, "{}\n", content)?;
                        if !wing.is_empty() || !room.is_empty() {
                            writeln!(f, "**Wing:** {} | **Room:** {}", wing, room)?;
                        }
                        writeln!(f, "**Filed:** {} | **Source:** {}", filed_at, source)?;
                        writeln!(f, "---\n")?;
                    }
                    _ => {
                        anyhow::bail!("unknown export format '{}'", format);
                    }
                }
            }
        }

        Ok(())
    }

    /// Migrate from a legacy JSON file containing `HashMap<String, DocumentEntry>`.
    ///
    /// Reads the JSON, batch-inserts all entries into SQLite, and
    /// returns the number of migrated drawers. If the store is
    /// non-empty, migration is skipped (assumed already migrated).
    pub fn migrate_from_json(&self, json_path: &Path) -> Result<usize> {
        if !self.is_empty() {
            info!(
                "drawer store already has {} drawers; skipping JSON migration",
                self.len()
            );
            return Ok(0);
        }

        if !json_path.exists() {
            anyhow::bail!("JSON file not found: {}", json_path.display());
        }

        let content = std::fs::read_to_string(json_path)
            .with_context(|| format!("reading {}", json_path.display()))?;
        let docs: HashMap<String, DocumentEntry> = serde_json::from_str(&content)
            .with_context(|| format!("parsing {}", json_path.display()))?;

        if docs.is_empty() {
            info!("JSON file is empty; nothing to migrate");
            return Ok(0);
        }

        let total = docs.len();
        info!("migrating {} drawers from {} to SQLite", total, json_path.display());

        // Prepare batch items
        let batch_size = 500;
        let items: Vec<_> = docs
            .iter()
            .map(|(id, entry)| {
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
                let source_file = entry
                    .metadata
                    .get("source_file")
                    .and_then(|v| v.as_str());
                let source_mtime = entry
                    .metadata
                    .get("source_mtime")
                    .and_then(|v| v.as_f64());

                (
                    id.as_str(),
                    entry.content.as_str(),
                    &entry.metadata,
                    wing,
                    room,
                    source_file,
                    source_mtime,
                )
            })
            .collect();

        // Insert in batches
        for chunk in items.chunks(batch_size) {
            self.insert_batch(chunk)?;
        }

        info!("migrated {} drawers to SQLite", total);
        Ok(total)
    }

    /// Check if FTS5 is available and the drawers_fts table exists.
    pub fn fts5_available(&self) -> bool {
        let guard = self.conn.lock().expect("conn");
        guard
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='drawers_fts' LIMIT 1",
                [],
                |_| Ok(1),
            )
            .is_ok()
    }

    /// Count drawers matching a wing and/or room filter.
    pub fn count_filtered(&self, wing: Option<&str>, room: Option<&str>) -> Result<usize> {
        let mut sql = String::from("SELECT COUNT(*) FROM drawers WHERE 1=1");
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(w) = wing {
            sql.push_str(" AND wing = ?");
            param_values.push(Box::new(w.to_string()));
        }
        if let Some(r) = room {
            sql.push_str(" AND room = ?");
            param_values.push(Box::new(r.to_string()));
        }

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();

        let count: i64 = self.conn.lock().expect("conn").query_row(&sql, params_refs.as_slice(), |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Get the source_file for a given drawer ID.
    pub fn get_source_file(&self, id: &str) -> Result<Option<String>> {
        let guard = self.conn.lock().expect("conn");
        let mut stmt = guard
            .prepare("SELECT source_file FROM drawers WHERE id = ?1")?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(row.get(0)?)
        } else {
            Ok(None)
        }
    }
}

/// Build an FTS5 query string from user input.
///
/// Escapes special FTS5 characters and joins terms with AND for
/// precision matching. Empty/malformed terms are filtered out.
fn build_fts_query(user_query: &str) -> String {
    // FTS5 special characters: ^, *, ", :, ~, (, ), +
    // We escape by wrapping each term in double quotes
    let terms: Vec<String> = user_query
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| {
            // Escape any double quotes in the term
            let escaped = t.replace('"', "\"\"");
            format!("\"{}\"", escaped)
        })
        .collect();

    if terms.is_empty() {
        return String::new();
    }

    terms.join(" AND ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_creates_schema() {
        let temp = tempfile::tempdir().unwrap();
        let store = DrawerStore::open(temp.path()).unwrap();
        assert!(store.is_empty());

        // Verify schema exists
        let has_drawers: bool = store
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='drawers'",
                [],
                |row| row.get(0),
            )
            .is_ok();
        assert!(has_drawers);

        let has_fts: bool = store
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='drawers_fts'",
                [],
                |row| row.get(0),
            )
            .is_ok();
        assert!(has_fts);
    }

    #[test]
    fn test_insert_and_count() {
        let temp = tempfile::tempdir().unwrap();
        let store = DrawerStore::open(temp.path()).unwrap();
        assert_eq!(store.len(), 0);

        let mut meta = HashMap::new();
        meta.insert("key1".to_string(), Value::String("val1".to_string()));

        store
            .insert("test-1", "hello world", &meta, "wing1", "room1", None, None)
            .unwrap();
        assert_eq!(store.len(), 1);

        store
            .insert("test-2", "foo bar", &HashMap::new(), "", "", None, None)
            .unwrap();
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn test_fts_search() {
        let temp = tempfile::tempdir().unwrap();
        let store = DrawerStore::open(temp.path()).unwrap();

        store
            .insert("d1", "the quick brown fox", &HashMap::new(), "animals", "mammals", None, None)
            .unwrap();
        store
            .insert("d2", "jumped over the lazy dog", &HashMap::new(), "animals", "mammals", None, None)
            .unwrap();
        store
            .insert("d3", "Rust programming language", &HashMap::new(), "tech", "languages", None, None)
            .unwrap();

        // Search for "fox" should find d1
        let results = store.search("fox", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "d1");

        // Search for "dog" should find d2
        let results = store.search("dog", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "d2");

        // Search for "Rust" should find d3
        let results = store.search("Rust", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "d3");

        // Search for "quick fox" (AND) should find d1
        let results = store.search("quick fox", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "d1");
    }

    #[test]
    fn test_get_by_id() {
        let temp = tempfile::tempdir().unwrap();
        let store = DrawerStore::open(temp.path()).unwrap();

        let mut meta = HashMap::new();
        meta.insert("source".to_string(), Value::String("test.txt".to_string()));

        store
            .insert("my-id", "some content", &meta, "w1", "r1", Some("src.txt"), Some(12345.0))
            .unwrap();

        let result = store.get_by_id("my-id").unwrap();
        assert!(result.is_some());
        let (content, metadata) = result.unwrap();
        assert_eq!(content, "some content");
        assert_eq!(
            metadata.get("source").and_then(|v| v.as_str()),
            Some("test.txt")
        );
        assert_eq!(metadata.get("wing").and_then(|v| v.as_str()), Some("w1"));
        assert_eq!(metadata.get("room").and_then(|v| v.as_str()), Some("r1"));

        // Non-existent ID
        let result = store.get_by_id("nope").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_delete() {
        let temp = tempfile::tempdir().unwrap();
        let store = DrawerStore::open(temp.path()).unwrap();

        store
            .insert("del-me", "to be deleted", &HashMap::new(), "", "", None, None)
            .unwrap();
        assert_eq!(store.len(), 1);

        assert!(store.delete("del-me").unwrap());
        assert_eq!(store.len(), 0);

        // Deleting non-existent returns false
        assert!(!store.delete("nope").unwrap());
    }

    #[test]
    fn test_delete_by_source() {
        let temp = tempfile::tempdir().unwrap();
        let store = DrawerStore::open(temp.path()).unwrap();

        store
            .insert("a", "content a", &HashMap::new(), "", "", Some("src1"), None)
            .unwrap();
        store
            .insert("b", "content b", &HashMap::new(), "", "", Some("src1"), None)
            .unwrap();
        store
            .insert("c", "content c", &HashMap::new(), "", "", Some("src2"), None)
            .unwrap();
        assert_eq!(store.len(), 3);

        assert_eq!(store.delete_by_source("src1").unwrap(), 2);
        assert_eq!(store.len(), 1);
        assert!(store.get_by_id("c").unwrap().is_some());
    }

    #[test]
    fn test_get_all_filtered() {
        let temp = tempfile::tempdir().unwrap();
        let store = DrawerStore::open(temp.path()).unwrap();

        store
            .insert("a", "content a", &HashMap::new(), "wing1", "room1", None, None)
            .unwrap();
        store
            .insert("b", "content b", &HashMap::new(), "wing1", "room2", None, None)
            .unwrap();
        store
            .insert("c", "content c", &HashMap::new(), "wing2", "room1", None, None)
            .unwrap();

        // All
        let all = store.get_all(None, None, 10).unwrap();
        assert_eq!(all.len(), 3);

        // Filter by wing
        let wing1 = store.get_all(Some("wing1"), None, 10).unwrap();
        assert_eq!(wing1.len(), 2);

        // Filter by wing + room
        let specific = store.get_all(Some("wing1"), Some("room1"), 10).unwrap();
        assert_eq!(specific.len(), 1);
    }

    #[test]
    fn test_load_all_to_hashmap() {
        let temp = tempfile::tempdir().unwrap();
        let store = DrawerStore::open(temp.path()).unwrap();

        let mut meta = HashMap::new();
        meta.insert("extra".to_string(), Value::String("value".to_string()));

        store
            .insert("id1", "hello", &meta, "w1", "r1", None, None)
            .unwrap();
        store
            .insert("id2", "world", &HashMap::new(), "", "", None, None)
            .unwrap();

        let map = store.load_all_to_hashmap().unwrap();
        assert_eq!(map.len(), 2);

        let entry1 = map.get("id1").unwrap();
        assert_eq!(entry1.content, "hello");
        assert_eq!(
            entry1.metadata.get("wing").and_then(|v| v.as_str()),
            Some("w1")
        );
        assert_eq!(
            entry1.metadata.get("extra").and_then(|v| v.as_str()),
            Some("value")
        );
    }

    #[test]
    fn test_batch_insert() {
        let temp = tempfile::tempdir().unwrap();
        let store = DrawerStore::open(temp.path()).unwrap();

        let items = vec![
            ("a", "alpha", &HashMap::new(), "w1", "r1", None as Option<&str>, None as Option<f64>),
            ("b", "beta", &HashMap::new(), "w1", "r1", None, None),
            ("c", "gamma", &HashMap::new(), "w1", "r2", None, None),
        ];

        store.insert_batch(&items).unwrap();
        assert_eq!(store.len(), 3);
    }

    #[test]
    fn test_count_filtered() {
        let temp = tempfile::tempdir().unwrap();
        let store = DrawerStore::open(temp.path()).unwrap();

        store
            .insert("a", "a", &HashMap::new(), "w1", "r1", None, None)
            .unwrap();
        store
            .insert("b", "b", &HashMap::new(), "w1", "r1", None, None)
            .unwrap();
        store
            .insert("c", "c", &HashMap::new(), "w1", "r2", None, None)
            .unwrap();

        assert_eq!(store.count_filtered(None, None).unwrap(), 3);
        assert_eq!(store.count_filtered(Some("w1"), None).unwrap(), 3);
        assert_eq!(store.count_filtered(Some("w1"), Some("r1")).unwrap(), 2);
        assert_eq!(store.count_filtered(Some("w1"), Some("r2")).unwrap(), 1);
        assert_eq!(store.count_filtered(Some("w2"), None).unwrap(), 0);
    }

    #[test]
    fn test_fts5_available() {
        let temp = tempfile::tempdir().unwrap();
        let store = DrawerStore::open(temp.path()).unwrap();
        assert!(store.fts5_available());
    }

    #[test]
    fn test_migrate_from_json() {
        let temp = tempfile::tempdir().unwrap();

        // Create a legacy JSON file
        let mut docs = HashMap::new();
        let mut meta = HashMap::new();
        meta.insert("wing".to_string(), Value::String("test".to_string()));
        meta.insert("room".to_string(), Value::String("migration".to_string()));
        docs.insert(
            "legacy-1".to_string(),
            DocumentEntry {
                content: "legacy content".to_string(),
                metadata: meta.clone(),
            },
        );
        docs.insert(
            "legacy-2".to_string(),
            DocumentEntry {
                content: "more legacy".to_string(),
                metadata: meta,
            },
        );

        let json_path = temp.path().join("legacy.json");
        let json_content = serde_json::to_string_pretty(&docs).unwrap();
        std::fs::write(&json_path, &json_content).unwrap();

        // Migrate
        let store = DrawerStore::open(temp.path()).unwrap();
        let count = store.migrate_from_json(&json_path).unwrap();
        assert_eq!(count, 2);
        assert_eq!(store.len(), 2);

        // Verify data
        let (content, _) = store.get_by_id("legacy-1").unwrap().unwrap();
        assert_eq!(content, "legacy content");

        // Second migration should be a no-op
        let count2 = store.migrate_from_json(&json_path).unwrap();
        assert_eq!(count2, 0);
    }
}
