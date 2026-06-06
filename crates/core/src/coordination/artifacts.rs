//! Artifact store for large payloads in multi-agent coordination.
//!
//! When content exceeds a threshold (e.g., 2KB), it's stored as a separate
//! artifact file rather than inline in signals or actions.

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Retention policy for artifacts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Retention {
    /// Delete after session ends.
    Ephemeral,
    /// Delete after swarm ends.
    Session,
    /// Delete when parent action completes.
    UntilCompletion,
    /// Keep forever.
    Persistent,
}

impl Retention {
    pub fn as_str(&self) -> &'static str {
        match self {
            Retention::Ephemeral => "ephemeral",
            Retention::Session => "session",
            Retention::UntilCompletion => "until_completion",
            Retention::Persistent => "persistent",
        }
    }
}

/// A stored artifact for large payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub category: String,
    pub entity_id: String,
    pub content: String,
    pub size_bytes: usize,
    pub retention: Retention,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Artifact store using file-based storage.
pub struct ArtifactStore {
    base_dir: PathBuf,
}

impl ArtifactStore {
    /// Create a new artifact store at the given base directory.
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Store an artifact. Returns the artifact ID.
    pub fn store(
        &self,
        category: &str,
        entity_id: &str,
        content: &str,
        retention: Retention,
    ) -> Result<String> {
        let id = format!("art-{}", uuid::Uuid::new_v4());
        let size_bytes = content.len();

        let expires_at = match retention {
            Retention::Ephemeral => Some(Utc::now() + Duration::hours(1)),
            Retention::Session => Some(Utc::now() + Duration::hours(24)),
            Retention::UntilCompletion => None, // cleaned up when parent completes
            Retention::Persistent => None,
        };

        let artifact = Artifact {
            id: id.clone(),
            category: category.to_string(),
            entity_id: entity_id.to_string(),
            content: content.to_string(),
            size_bytes,
            retention,
            created_at: Utc::now(),
            expires_at,
        };

        let dir = self.base_dir.join("artifacts").join(category);
        fs::create_dir_all(&dir)?;

        let path = dir.join(format!("{}.json", id));
        let json = serde_json::to_string_pretty(&artifact)?;
        fs::write(&path, json)?;

        Ok(id)
    }

    /// Read an artifact by ID.
    pub fn read(&self, artifact_id: &str) -> Result<Option<Artifact>> {
        // Search across all category directories
        let artifacts_dir = self.base_dir.join("artifacts");
        if !artifacts_dir.exists() {
            return Ok(None);
        }

        for entry in fs::read_dir(&artifacts_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }

            let path = entry.path().join(format!("{}.json", artifact_id));
            if path.exists() {
                let json = fs::read_to_string(&path)?;
                let artifact: Artifact = serde_json::from_str(&json)?;
                return Ok(Some(artifact));
            }
        }

        Ok(None)
    }

    /// Delete an artifact by ID.
    pub fn delete(&self, artifact_id: &str) -> Result<()> {
        let artifacts_dir = self.base_dir.join("artifacts");
        if !artifacts_dir.exists() {
            return Ok(());
        }

        for entry in fs::read_dir(&artifacts_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }

            let path = entry.path().join(format!("{}.json", artifact_id));
            if path.exists() {
                fs::remove_file(&path)?;
                return Ok(());
            }
        }

        Ok(())
    }

    /// Cleanup expired artifacts. Returns count of deleted artifacts.
    pub fn cleanup(&self) -> Result<usize> {
        let artifacts_dir = self.base_dir.join("artifacts");
        if !artifacts_dir.exists() {
            return Ok(0);
        }

        let now = Utc::now();
        let mut count = 0;

        for entry in fs::read_dir(&artifacts_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }

            for file_entry in fs::read_dir(entry.path())? {
                let file_entry = file_entry?;
                let path = file_entry.path();
                if !path.extension().map_or(false, |e| e == "json") {
                    continue;
                }

                let json = fs::read_to_string(&path)?;
                if let Ok(artifact) = serde_json::from_str::<Artifact>(&json) {
                    if let Some(expires_at) = artifact.expires_at {
                        if now > expires_at {
                            fs::remove_file(&path)?;
                            count += 1;
                        }
                    }
                }
            }
        }

        Ok(count)
    }

    /// List artifacts for a specific entity.
    pub fn by_entity(&self, entity_id: &str) -> Result<Vec<Artifact>> {
        let artifacts_dir = self.base_dir.join("artifacts");
        if !artifacts_dir.exists() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();

        for entry in fs::read_dir(&artifacts_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }

            for file_entry in fs::read_dir(entry.path())? {
                let file_entry = file_entry?;
                let path = file_entry.path();
                if !path.extension().map_or(false, |e| e == "json") {
                    continue;
                }

                let json = fs::read_to_string(&path)?;
                if let Ok(artifact) = serde_json::from_str::<Artifact>(&json) {
                    if artifact.entity_id == entity_id {
                        results.push(artifact);
                    }
                }
            }
        }

        Ok(results)
    }

    /// Check if content should be stored as an artifact (>2KB).
    pub fn should_artifactize(content: &str) -> bool {
        content.len() > 2048
    }

    /// Artifactize content: store as artifact and return reference string.
    pub fn artifactize(
        &self,
        category: &str,
        entity_id: &str,
        content: &str,
        retention: Retention,
    ) -> Result<String> {
        if !Self::should_artifactize(content) {
            return Ok(content.to_string());
        }

        let artifact_id = self.store(category, entity_id, content, retention)?;
        Ok(format!(
            "[Artifact: {} bytes, id: {}]",
            content.len(),
            artifact_id
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_store() -> (ArtifactStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = ArtifactStore::new(dir.path().to_path_buf());
        (store, dir)
    }

    #[test]
    fn test_store_and_read() {
        let (store, _dir) = create_store();

        let id = store
            .store("signal_body", "sig-1", "Hello world", Retention::Session)
            .unwrap();

        let artifact = store.read(&id).unwrap().unwrap();
        assert_eq!(artifact.content, "Hello world");
        assert_eq!(artifact.entity_id, "sig-1");
        assert_eq!(artifact.category, "signal_body");
    }

    #[test]
    fn test_delete() {
        let (store, _dir) = create_store();

        let id = store
            .store("test", "entity-1", "data", Retention::Ephemeral)
            .unwrap();

        store.delete(&id).unwrap();
        assert!(store.read(&id).unwrap().is_none());
    }

    #[test]
    fn test_by_entity() {
        let (store, _dir) = create_store();

        store
            .store("signal_body", "sig-1", "content1", Retention::Session)
            .unwrap();
        store
            .store("signal_body", "sig-1", "content2", Retention::Session)
            .unwrap();
        store
            .store("signal_body", "sig-2", "content3", Retention::Session)
            .unwrap();

        let artifacts = store.by_entity("sig-1").unwrap();
        assert_eq!(artifacts.len(), 2);
    }

    #[test]
    fn test_should_artifactize() {
        assert!(!ArtifactStore::should_artifactize("short"));
        assert!(ArtifactStore::should_artifactize(&"x".repeat(3000)));
    }

    #[test]
    fn test_artifactize_small() {
        let (store, _dir) = create_store();

        let result = store
            .artifactize("test", "entity-1", "small content", Retention::Session)
            .unwrap();

        // Small content returned as-is
        assert_eq!(result, "small content");
    }

    #[test]
    fn test_artifactize_large() {
        let (store, _dir) = create_store();

        let large_content = "x".repeat(3000);
        let result = store
            .artifactize("test", "entity-1", &large_content, Retention::Session)
            .unwrap();

        // Large content stored as artifact
        assert!(result.contains("[Artifact:"));
        assert!(result.contains("id:"));
    }

    #[test]
    fn test_cleanup_expired() {
        let (store, _dir) = create_store();

        // Store with ephemeral retention (1 hour TTL)
        store
            .store("test", "entity-1", "data", Retention::Ephemeral)
            .unwrap();

        // Cleanup won't delete fresh artifacts
        let cleaned = store.cleanup().unwrap();
        assert_eq!(cleaned, 0);
    }
}
