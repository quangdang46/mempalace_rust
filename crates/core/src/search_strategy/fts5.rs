//! FTS5 strategy — SQLite FTS5 with trigram tokenizer for CJK support.
//!
//! Default strategy. 0MB extra, instant, lexical matching.
//! Falls back to naive search if FTS5 tables not initialized.

use super::naive::NaiveJaccardStrategy;
use super::traits::{SearchHit, SearchStrategy};
use crate::palace_db::PalaceDb;
use anyhow::Result;
use rusqlite::Connection;

pub struct Fts5Strategy;

impl Fts5Strategy {
    pub fn new() -> Self {
        Self
    }
}

impl Default for Fts5Strategy {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchStrategy for Fts5Strategy {
    fn name(&self) -> &str {
        "fts5"
    }

    fn search(&self, query: &str, db: &PalaceDb, n: usize) -> Result<Vec<SearchHit>> {
        // Try FTS5 first; fall back to naive on error
        match fts5_search(db, query, n) {
            Ok(hits) if !hits.is_empty() => Ok(hits),
            Ok(_) | Err(_) => {
                // Fall back to naive Jaccard
                NaiveJaccardStrategy::new().search(query, db, n)
            }
        }
    }
}

fn fts5_search(db: &PalaceDb, query: &str, n: usize) -> Result<Vec<SearchHit>> {
    let conn = Connection::open(db.path().join("mempalace.db"))?;
    // Use FTS5 with BM25 ranking
    let mut stmt = conn.prepare(
        "SELECT id, bm25(drawers_fts) AS score
         FROM drawers_fts
         WHERE drawers_fts MATCH ?1
         ORDER BY score
         LIMIT ?2",
    )?;
    let hits: Vec<SearchHit> = stmt
        .query_map(rusqlite::params![query, n as i64], |row| {
            Ok(SearchHit {
                id: row.get(0)?,
                score: -row.get::<_, f64>(1)?, // negate so higher=better
                metadata: None,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(hits)
}
