use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::onnx_embed::OnnxModel;

pub const DEFAULT_COLLECTION_NAME: &str = "mempalace_drawers";
pub const DEFAULT_COMPRESSED_COLLECTION_NAME: &str = "mempalace_compressed";

pub struct PalaceDb {
    documents: HashMap<String, DocumentEntry>,
    palace_path: PathBuf,
    collection_name: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct DocumentEntry {
    content: String,
    metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct QueryResult {
    pub ids: Vec<String>,
    pub documents: Vec<String>,
    pub distances: Vec<f64>,
    pub metadatas: Vec<HashMap<String, serde_json::Value>>,
}

pub struct EmbeddingDb {
    embedder: Arc<OnnxModel>,
    hnsw: embedvec::HnswIndex,
    #[allow(dead_code)]
    documents: Vec<(String, String)>,
    storage: embedvec::VectorStorage,
}

impl PalaceDb {
    pub fn open(palace_path: &std::path::Path) -> anyhow::Result<Self> {
        Self::open_collection(palace_path, DEFAULT_COLLECTION_NAME)
    }

    pub fn open_collection(
        palace_path: &std::path::Path,
        collection_name: &str,
    ) -> anyhow::Result<Self> {
        let collection_name = collection_name.to_string();
        let docs_path = palace_path.join(format!("{}.json", collection_name));

        let documents = if docs_path.exists() {
            let content = std::fs::read_to_string(&docs_path)?;
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            HashMap::new()
        };

        Ok(Self {
            documents,
            palace_path: palace_path.to_path_buf(),
            collection_name,
        })
    }

    pub async fn query(
        &self,
        query_text: &str,
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
    ) -> anyhow::Result<Vec<QueryResult>> {
        self.query_sync(query_text, wing, room, n_results)
    }

    pub fn query_sync(
        &self,
        query_text: &str,
        wing: Option<&str>,
        room: Option<&str>,
        n_results: usize,
    ) -> anyhow::Result<Vec<QueryResult>> {
        let query_lower = query_text.to_lowercase();

        let mut results: Vec<(String, f64, &DocumentEntry)> = self
            .documents
            .iter()
            .filter_map(|(id, entry)| {
                if let Some(w) = wing {
                    let entry_wing = entry
                        .metadata
                        .get("wing")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if entry_wing != w {
                        return None;
                    }
                }
                if let Some(r) = room {
                    let entry_room = entry
                        .metadata
                        .get("room")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if entry_room != r {
                        return None;
                    }
                }
                let similarity = naive_similarity(&query_lower, &entry.content.to_lowercase());
                if similarity > 0.05 {
                    Some((id.clone(), similarity, entry))
                } else {
                    None
                }
            })
            .collect();

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(n_results);

        let query_results: Vec<QueryResult> = results
            .into_iter()
            .map(|(id, similarity, entry)| {
                let mut metadata = entry.metadata.clone();
                metadata.insert("distance".to_string(), serde_json::json!(1.0 - similarity));

                QueryResult {
                    ids: vec![id],
                    documents: vec![entry.content.clone()],
                    distances: vec![1.0 - similarity],
                    metadatas: vec![metadata],
                }
            })
            .collect();

        Ok(query_results)
    }

    pub fn add(
        &mut self,
        documents: &[(&str, &str)],
        metadata: &[&[(&str, &str)]],
    ) -> anyhow::Result<()> {
        for ((id, content), meta) in documents.iter().zip(metadata.iter()) {
            let meta_map: HashMap<String, serde_json::Value> = meta
                .iter()
                .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
                .collect();

            self.documents.insert(
                id.to_string(),
                DocumentEntry {
                    content: content.to_string(),
                    metadata: meta_map,
                },
            );
        }

        // Don't auto-save on every add - caller should call flush() when done batching
        Ok(())
    }

    pub fn upsert_documents(
        &mut self,
        documents: &[(String, String, HashMap<String, serde_json::Value>)],
    ) -> anyhow::Result<()> {
        for (id, content, metadata) in documents {
            self.documents.insert(
                id.clone(),
                DocumentEntry {
                    content: content.clone(),
                    metadata: metadata.clone(),
                },
            );
        }

        Ok(())
    }

    pub fn delete_id(&mut self, id: &str) -> anyhow::Result<bool> {
        let removed = self.documents.remove(id).is_some();
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    pub fn file_already_mined(&self, source_file: &str, check_mtime: bool) -> bool {
        let Some(entry) = self.documents.values().find(|entry| {
            entry.metadata.get("source_file").and_then(|v| v.as_str()) == Some(source_file)
        }) else {
            return false;
        };

        if !check_mtime {
            return true;
        }

        let Some(stored_mtime) = entry
            .metadata
            .get("source_mtime")
            .and_then(|v| v.as_str())
            .and_then(|v| v.parse::<f64>().ok())
        else {
            return false;
        };

        let Ok(metadata) = std::fs::metadata(source_file) else {
            return false;
        };
        let Ok(modified) = metadata.modified() else {
            return false;
        };
        let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) else {
            return false;
        };

        (duration.as_secs_f64() - stored_mtime).abs() < f64::EPSILON
    }

    pub fn flush(&mut self) -> anyhow::Result<()> {
        self.save()
    }

    pub fn complete_test_setup(&mut self) -> anyhow::Result<()> {
        self.flush()
    }

    fn save(&self) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.palace_path)?;

        let docs_path = self
            .palace_path
            .join(format!("{}.json", self.collection_name));
        let content = serde_json::to_string_pretty(&self.documents)?;
        std::fs::write(docs_path, content)?;

        Ok(())
    }

    pub(crate) fn _get_document(&self, id: &str) -> Option<&DocumentEntry> {
        self.documents.get(id)
    }

    /// Get metadata for a document by ID.
    pub fn get_document_metadata(&self, id: &str) -> Option<&HashMap<String, serde_json::Value>> {
        self.documents.get(id).map(|e| &e.metadata)
    }

    /// Get documents by their IDs. Returns only the IDs that exist.
    pub fn get_documents(&self, ids: &[String]) -> Vec<String> {
        ids.iter()
            .filter(|id| self.documents.contains_key(id.as_str()))
            .cloned()
            .collect()
    }

    /// Get all documents that have a matching session_id in their metadata.
    /// Returns vector of (id, content, metadata) tuples.
    pub fn get_documents_by_session(
        &self,
        session_id: &str,
    ) -> Vec<(String, String, HashMap<String, serde_json::Value>)> {
        self.documents
            .iter()
            .filter(|(_, entry)| {
                entry
                    .metadata
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    == Some(session_id)
            })
            .map(|(id, entry)| (id.clone(), entry.content.clone(), entry.metadata.clone()))
            .collect()
    }

    pub fn count(&self) -> usize {
        self.documents.len()
    }

    /// Get all documents, optionally filtered by wing and/or room.
    /// Returns results sorted by importance (from metadata or distance).
    pub fn get_all(
        &self,
        wing: Option<&str>,
        room: Option<&str>,
        limit: usize,
    ) -> Vec<QueryResult> {
        let mut entries: Vec<(&String, &DocumentEntry)> = self
            .documents
            .iter()
            .filter(|(_, entry)| {
                if let Some(w) = wing {
                    let entry_wing = entry
                        .metadata
                        .get("wing")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if entry_wing != w {
                        return false;
                    }
                }
                if let Some(r) = room {
                    let entry_room = entry
                        .metadata
                        .get("room")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if entry_room != r {
                        return false;
                    }
                }
                true
            })
            .collect();

        // Sort by importance metadata if available, otherwise by order added
        entries.sort_by(|(id_a, entry_a), (id_b, entry_b)| {
            let imp_a = entry_a
                .metadata
                .get("importance")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.5);
            let imp_b = entry_b
                .metadata
                .get("importance")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.5);
            imp_b
                .partial_cmp(&imp_a)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| id_a.cmp(id_b))
        });

        entries.truncate(limit);

        let query_results: Vec<QueryResult> = entries
            .into_iter()
            .map(|(id, entry)| QueryResult {
                ids: vec![id.clone()],
                documents: vec![entry.content.clone()],
                distances: vec![0.0],
                metadatas: vec![entry.metadata.clone()],
            })
            .collect();

        query_results
    }
}

impl EmbeddingDb {
    pub fn new(dimension: usize) -> anyhow::Result<Self> {
        let embedder = OnnxModel::load()?;
        Self::with_embedder(Arc::new(embedder), dimension)
    }

    pub fn with_embedder(embedder: Arc<OnnxModel>, dimension: usize) -> anyhow::Result<Self> {
        let hnsw = embedvec::HnswIndex::new(16, 200, embedvec::Distance::Cosine);
        let storage = embedvec::VectorStorage::new(dimension, embedvec::Quantization::None);
        Ok(Self {
            embedder,
            hnsw,
            documents: Vec::new(),
            storage,
        })
    }

    pub fn add(&mut self, id: &str, text: &str) -> anyhow::Result<usize> {
        let embedding = self.embed(text)?;
        let idx = self.documents.len();
        self.documents.push((id.to_string(), text.to_string()));
        self.storage.add(&embedding, None)?;
        self.hnsw.insert(idx, &embedding, &self.storage, None)?;
        Ok(idx)
    }

    pub fn add_batch(&mut self, items: &[(String, String)]) -> anyhow::Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let texts: Vec<&str> = items.iter().map(|(_, t)| t.as_str()).collect();
        let embeddings = self.embedder.encode_batch(&texts, true)?;
        let start_idx = self.documents.len();
        for (i, (id, text)) in items.iter().enumerate() {
            self.documents.push((id.clone(), text.clone()));
            // Normalize ONNX embeddings before storing (ONNX model returns unnormalized)
            let normalized = normalize_embedding(&embeddings[i]);
            self.storage.add(&normalized, None)?;
            self.hnsw
                .insert(start_idx + i, &normalized, &self.storage, None)?;
        }
        Ok(())
    }

    pub fn query(&self, query_text: &str, n_results: usize) -> anyhow::Result<Vec<(f32, usize)>> {
        let query_embedding = self.embed(query_text)?;
        let normalized_query = normalize_embedding(&query_embedding);
        let results = self
            .hnsw
            .search(&normalized_query, n_results, 1024, &self.storage, None)?;
        Ok(results.into_iter().map(|(id, dist)| (dist, id)).collect())
    }

    pub fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let embedding = self.embedder.encode(text)?;
        Ok(embedding)
    }
}

fn normalize_embedding(embedding: &[f32]) -> Vec<f32> {
    let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        return embedding.to_vec();
    }
    embedding.iter().map(|x| x / norm).collect()
}

fn naive_similarity(query: &str, content: &str) -> f64 {
    let query_words: std::collections::HashSet<_> = query.split_whitespace().collect();
    let content_words: std::collections::HashSet<_> = content.split_whitespace().collect();

    if query_words.is_empty() || content_words.is_empty() {
        return 0.0;
    }

    let intersection = query_words.intersection(&content_words).count();
    let union = query_words.union(&content_words).count();

    intersection as f64 / union as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_naive_similarity() {
        let sim = naive_similarity("hello world", "hello world");
        assert!((sim - 1.0).abs() < 1e-6);

        let sim = naive_similarity("hello world", "hello");
        assert!(sim > 0.3 && sim < 0.7);

        let sim = naive_similarity("hello world", "completely different");
        assert!(sim < 0.1);
    }
}
