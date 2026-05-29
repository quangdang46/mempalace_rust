use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::Path;

use super::image_store::{delete_image, touch_image};

pub struct ImageRefStore {
    conn: Connection,
}

impl ImageRefStore {
    pub fn new(conn: Connection) -> Result<Self> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS image_refs (
                file_path TEXT PRIMARY KEY,
                ref_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            )",
            [],
        )?;
        Ok(Self { conn })
    }

    pub fn get_ref_count<P: AsRef<Path>>(&self, path: P) -> Result<u64> {
        let path_str = path.as_ref().to_string_lossy();
        let count: Result<u64, _> = self.conn.query_row(
            "SELECT ref_count FROM image_refs WHERE file_path = ?1",
            params![path_str],
            |row| row.get(0),
        );
        Ok(count.unwrap_or(0))
    }

    pub fn increment_ref<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path_str = path.as_ref().to_string_lossy().to_string();
        let current = self.get_ref_count(&path_str)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO image_refs (file_path, ref_count) VALUES (?1, ?2)",
            params![path_str, current + 1],
        )?;
        touch_image(&path_str)?;
        Ok(())
    }

    pub fn decrement_ref<P: AsRef<Path>>(&self, path: P) -> Result<u64> {
        let path_str = path.as_ref().to_string_lossy().to_string();
        let current = self.get_ref_count(&path_str)?;
        if current <= 1 {
            self.conn.execute(
                "DELETE FROM image_refs WHERE file_path = ?1",
                params![path_str],
            )?;
            let deleted_bytes = delete_image(&path_str)?;
            Ok(deleted_bytes)
        } else {
            self.conn.execute(
                "UPDATE image_refs SET ref_count = ?1 WHERE file_path = ?2",
                params![current - 1, path_str],
            )?;
            Ok(0)
        }
    }

    pub fn list_unreferenced(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT file_path FROM image_refs WHERE ref_count <= 0"
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut paths = Vec::new();
        for row in rows {
            paths.push(row?);
        }
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> ImageRefStore {
        let conn = Connection::open_in_memory().unwrap();
        ImageRefStore::new(conn).unwrap()
    }

    #[test]
    fn test_get_ref_count_empty() {
        let store = test_store();
        assert_eq!(store.get_ref_count("/tmp/test.png").unwrap(), 0);
    }

    #[test]
    fn test_increment_ref() {
        let store = test_store();
        store.increment_ref("/tmp/test.png").unwrap();
        assert_eq!(store.get_ref_count("/tmp/test.png").unwrap(), 1);
    }

    #[test]
    fn test_increment_ref_twice() {
        let store = test_store();
        store.increment_ref("/tmp/test.png").unwrap();
        store.increment_ref("/tmp/test.png").unwrap();
        assert_eq!(store.get_ref_count("/tmp/test.png").unwrap(), 2);
    }

    #[test]
    fn test_decrement_ref() {
        let store = test_store();
        store.increment_ref("/tmp/test.png").unwrap();
        store.increment_ref("/tmp/test.png").unwrap();
        store.decrement_ref("/tmp/test.png").unwrap();
        assert_eq!(store.get_ref_count("/tmp/test.png").unwrap(), 1);
    }

    #[test]
    fn test_decrement_ref_to_zero() {
        let store = test_store();
        store.increment_ref("/tmp/test.png").unwrap();
        let deleted = store.decrement_ref("/tmp/test.png").unwrap();
        assert_eq!(store.get_ref_count("/tmp/test.png").unwrap(), 0);
        assert_eq!(deleted, 0); // file doesn't exist
    }

    #[test]
    fn test_list_unreferenced() {
        let store = test_store();
        store.increment_ref("/tmp/a.png").unwrap();
        store.decrement_ref("/tmp/a.png").unwrap();
        let unreferenced = store.list_unreferenced().unwrap();
        assert!(unreferenced.is_empty()); // deleted at ref_count <= 1
    }
}
