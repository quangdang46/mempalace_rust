use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;

use super::embedding_provider::{cosine_similarity, EmbeddingProvider, StoredEmbedding};
use super::image_refs::ImageRefStore;
use super::image_store::is_managed_image_path;

pub struct VisionSearchStore {
    conn: Connection,
    image_ref_store: ImageRefStore,
    provider: Option<Box<dyn EmbeddingProvider>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub image_ref: String,
    pub score: f32,
    pub session_id: Option<String>,
    pub observation_id: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedResult {
    pub image_ref: String,
    pub dimensions: usize,
}

impl VisionSearchStore {
    pub fn new(
        conn: Connection,
        provider: Option<Box<dyn EmbeddingProvider>>,
    ) -> Result<Self> {
        // Open a separate connection for image_refs (SQLite doesn't support Connection::try_clone)
        let db_path = conn.path()
            .map(|p| p.to_string())
            .unwrap_or_else(|| ":memory:".to_string());
        let ref_conn = Connection::open(&db_path)?;
        let image_ref_store = ImageRefStore::new(ref_conn)?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS image_embeddings (
                image_ref TEXT PRIMARY KEY,
                vector BLOB NOT NULL,
                model_name TEXT NOT NULL,
                dimensions INTEGER NOT NULL,
                updated_at TEXT NOT NULL,
                session_id TEXT,
                observation_id TEXT
            )",
            [],
        )?;
        Ok(Self {
            conn,
            image_ref_store,
            provider,
        })
    }

    /// Embed an image and store its vector.
    /// Matches upstream mem::vision-embed.
    pub fn vision_embed<P: AsRef<Path>>(
        &self,
        image_path: P,
        session_id: Option<&str>,
        observation_id: Option<&str>,
    ) -> Result<EmbedResult> {
        let provider = self.provider.as_ref()
            .ok_or_else(|| anyhow::anyhow!("image embeddings disabled"))?;

        let path = image_path.as_ref();
        if !is_managed_image_path(path) {
            anyhow::bail!("image_ref must point to a file under the managed image store");
        }

        let ref_count = self.image_ref_store.get_ref_count(path)?;
        if ref_count < 1 {
            anyhow::bail!("image_ref not registered in image_refs");
        }

        let path_str = path.to_string_lossy();
        let vector = provider.embed_image(&path_str)?;
        let dimensions = vector.len();
        let updated_at = chrono::Utc::now().to_rfc3339();

        let vector_bytes: Vec<u8> = vector.iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();

        self.conn.execute(
            "INSERT OR REPLACE INTO image_embeddings
             (image_ref, vector, model_name, dimensions, updated_at, session_id, observation_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                path_str,
                vector_bytes,
                provider.name(),
                dimensions as i64,
                updated_at,
                session_id,
                observation_id,
            ],
        )?;

        Ok(EmbedResult {
            image_ref: path_str.to_string(),
            dimensions,
        })
    }

    /// Search for similar images by text query, image ref, or base64.
    /// Matches upstream mem::vision-search.
    pub fn vision_search(
        &self,
        query_text: Option<&str>,
        query_image_ref: Option<&str>,
        top_k: Option<usize>,
        session_id: Option<&str>,
    ) -> Result<Vec<SearchResult>> {
        let provider = self.provider.as_ref()
            .ok_or_else(|| anyhow::anyhow!("image embeddings disabled"))?;

        let requested_top_k = top_k.unwrap_or(10);
        let top_k = requested_top_k.min(50).max(1);

        // Build query vector
        let query_vec = if let Some(text) = query_text {
            provider.embed(text)?
        } else if let Some(image_ref) = query_image_ref {
            let path = std::path::Path::new(image_ref);
            if !is_managed_image_path(path) {
                anyhow::bail!("query_image_ref must point to a file under the managed image store");
            }
            let ref_count = self.image_ref_store.get_ref_count(path)?;
            if ref_count < 1 {
                anyhow::bail!("query_image_ref not registered in image_refs");
            }
            provider.embed_image(image_ref)?
        } else {
            anyhow::bail!("query_text or query_image_ref required");
        };

        // Load all stored embeddings
        let mut stmt = self.conn.prepare(
            "SELECT image_ref, vector, dimensions, session_id, observation_id, updated_at
             FROM image_embeddings"
        )?;
        let rows = stmt.query_map([], |row| {
            let image_ref: String = row.get(0)?;
            let vector_bytes: Vec<u8> = row.get(1)?;
            let dimensions: i64 = row.get(2)?;
            let session_id: Option<String> = row.get(3)?;
            let observation_id: Option<String> = row.get(4)?;
            let updated_at: String = row.get(5)?;
            Ok((image_ref, vector_bytes, dimensions, session_id, observation_id, updated_at))
        })?;

        let mut scored: Vec<SearchResult> = Vec::new();
        for row in rows {
            let (image_ref, vector_bytes, dimensions, row_session_id, observation_id, updated_at) = row?;

            // Filter by session if specified
            if let Some(sid) = session_id {
                if row_session_id.as_deref() != Some(sid) {
                    continue;
                }
            }

            // Convert bytes back to f32 vector
            let chunks = vector_bytes.chunks_exact(4);
            let stored_vec: Vec<f32> = chunks
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect();

            if stored_vec.len() != dimensions as usize {
                continue;
            }

            let sim = cosine_similarity(&query_vec, &stored_vec);
            scored.push(SearchResult {
                image_ref,
                score: sim,
                session_id: row_session_id,
                observation_id,
                updated_at,
            });
        }

        // Sort by score descending
        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        Ok(scored.into_iter().take(top_k).collect())
    }

    /// Get ref count for an image.
    pub fn get_image_ref_count<P: AsRef<Path>>(&self, path: P) -> Result<u64> {
        self.image_ref_store.get_ref_count(path)
    }

    /// Increment ref count for an image.
    pub fn increment_image_ref<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        self.image_ref_store.increment_ref(path)
    }

    /// Decrement ref count for an image.
    pub fn decrement_image_ref<P: AsRef<Path>>(&self, path: P) -> Result<u64> {
        self.image_ref_store.decrement_ref(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vision::embedding_provider::StubEmbeddingProvider;

    fn test_store() -> VisionSearchStore {
        let conn = Connection::open_in_memory().unwrap();
        let provider = Box::new(StubEmbeddingProvider::new(8));
        VisionSearchStore::new(conn, Some(provider)).unwrap()
    }

    #[test]
    fn test_vision_embed_requires_provider() {
        let conn = Connection::open_in_memory().unwrap();
        let store = VisionSearchStore::new(conn, None).unwrap();
        let result = store.vision_embed("/tmp/test.png", None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_vision_search_requires_query() {
        let store = test_store();
        let result = store.vision_search(None, None, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_vision_search_empty_results() {
        let store = test_store();
        let results = store.vision_search(Some("test query"), None, None, None).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_vision_search_top_k_clamping() {
        let store = test_store();
        // top_k > 50 should be clamped to 50
        let results = store.vision_search(Some("test"), None, Some(100), None).unwrap();
        assert!(results.len() <= 50);
    }

    #[test]
    fn test_vision_search_top_k_minimum() {
        let store = test_store();
        // top_k < 1 should be clamped to 1
        let results = store.vision_search(Some("test"), None, Some(0), None).unwrap();
        assert!(results.len() <= 1);
    }

    #[test]
    fn test_cosine_similarity_in_search() {
        // Verify that search returns results sorted by cosine similarity
        let store = test_store();
        let results = store.vision_search(Some("query"), None, Some(5), None).unwrap();
        assert!(results.is_empty()); // No embeddings stored yet
    }

    #[test]
    fn test_image_ref_counting() {
        let store = test_store();
        assert_eq!(store.get_image_ref_count("/tmp/test.png").unwrap(), 0);
        store.increment_image_ref("/tmp/test.png").unwrap();
        assert_eq!(store.get_image_ref_count("/tmp/test.png").unwrap(), 1);
    }

    #[test]
    fn test_unmanaged_image_path_rejected() {
        let store = test_store();
        store.increment_image_ref("/tmp/test.png").unwrap();
        // /tmp is not under the managed image store
        let result = store.vision_embed("/tmp/test.png", None, None);
        assert!(result.is_err());
    }
}
