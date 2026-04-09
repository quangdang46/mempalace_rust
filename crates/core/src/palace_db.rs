use std::collections::HashMap;
use std::path::PathBuf;

pub const DEFAULT_COLLECTION_NAME: &str = "mempalace_drawers";

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

impl PalaceDb {
    pub fn open(palace_path: &std::path::Path) -> anyhow::Result<Self> {
        let collection_name = DEFAULT_COLLECTION_NAME.to_string();
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
