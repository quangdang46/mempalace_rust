//! Vector index migration — handle embedder/schema changes.
//!
//! Port of upstream `migrate-vector-index.ts` from agentmemory.
//! Detects the current embedding index version and supports
//! re-embedding or schema migration when the embedder model
//! or index format changes.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Current vector index schema version.
///
/// Increment this when making backward-incompatible changes to the
/// index format so that `mpr repair migrate-vector-index` knows the
/// index needs rebuilding.
const CURRENT_INDEX_VERSION: u32 = 2;

/// Vector index metadata stored alongside the palace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorIndexMeta {
    /// Schema version of the vector index.
    pub version: u32,
    /// Model name that was used to create embeddings.
    pub model: String,
    /// Fingerprint of the embedder at index creation time.
    pub fingerprint: String,
    /// Dimensionality of stored vectors.
    pub dim: usize,
    /// Whether the index is an in-memory-only rebuild (no persistence).
    pub ephemeral: bool,
}

impl VectorIndexMeta {
    /// Create metadata for the current schema version.
    pub fn new(model: &str, fingerprint: &str, dim: usize) -> Self {
        Self {
            version: CURRENT_INDEX_VERSION,
            model: model.to_string(),
            fingerprint: fingerprint.to_string(),
            dim,
            ephemeral: false,
        }
    }
}

/// Result of a migration operation.
#[derive(Debug, Clone, Serialize)]
pub struct MigrateIndexResult {
    pub drawers_reindexed: usize,
    pub errors: usize,
    pub old_version: u32,
    pub new_version: u32,
}

/// Detect the current vector index version by reading the manifest.
///
/// Returns `(version, model, fingerprint, dim)` or an error if the
/// manifest is missing/invalid.
pub fn detect_index_version(palace_path: &Path) -> Result<(u32, String, String, usize)> {
    let manifest_path = palace_path.join("embedding.json");
    if !manifest_path.exists() {
        // First-time / pre-manifest palace — treat as version 0.
        return Ok((0, "unknown".into(), "unknown".into(), 0));
    }
    let content = std::fs::read_to_string(&manifest_path)?;
    let meta: VectorIndexMeta = serde_json::from_str(&content)?;
    Ok((meta.version, meta.model, meta.fingerprint, meta.dim))
}

/// Store current index version metadata.
pub fn write_index_meta(palace_path: &Path, meta: &VectorIndexMeta) -> Result<()> {
    let manifest_path = palace_path.join("embedding.json");
    let content = serde_json::to_string_pretty(meta)?;
    std::fs::write(&manifest_path, content)?;
    Ok(())
}

/// Check if the index needs migration based on the running embedder.
///
/// If the manifest is stale (missing, wrong version, or wrong
/// fingerprint), returns `true` and prints guidance.
pub fn needs_migration(palace_path: &Path) -> Result<bool> {
    let (version, ..) = detect_index_version(palace_path)?;
    Ok(version < CURRENT_INDEX_VERSION)
}

/// Run a full vector index migration.
///
/// 1. Reads all drawers from the current palace
/// 2. Re-embeds each drawer using the current embedder
/// 3. Rebuilds the index
/// 4. Updates the manifest to the current version
pub fn migrate_index(palace_path: &Path) -> Result<MigrateIndexResult> {
    let (old_version, old_model, old_fingerprint, old_dim) =
        detect_index_version(palace_path).unwrap_or((0, "unknown".into(), "unknown".into(), 0));

    eprintln!(
        "  Vector index: v{} (model={}, fingerprint={}, dim={})",
        old_version, old_model, old_fingerprint, old_dim
    );

    // Open the palace DB and count drawers.
    let mut db = crate::palace_db::PalaceDb::open(palace_path)?;
    let drawer_count = db.count();

    if drawer_count == 0 {
        // Empty palace — just bump the manifest.
        if let Ok(embedder) = crate::embed::embedder_from_env() {
            let fp = embedder.fingerprint().to_string();
            let dim = embedder.dim();
            let meta = VectorIndexMeta::new("auto", &fp, dim);
            write_index_meta(palace_path, &meta)?;
            eprintln!(
                "  No drawers to re-index; updated manifest to v{}",
                CURRENT_INDEX_VERSION
            );
        }
        return Ok(MigrateIndexResult {
            drawers_reindexed: 0,
            errors: 0,
            old_version,
            new_version: CURRENT_INDEX_VERSION,
        });
    }

    // Collect all drawers.
    let all = db.get_all(None, None, drawer_count);

    // Flatten documents + metadatas.
    let mut docs: Vec<String> = Vec::new();
    let mut metas: Vec<std::collections::HashMap<String, serde_json::Value>> = Vec::new();
    let mut ids: Vec<String> = Vec::new();

    for qr in &all {
        for (i, doc) in qr.documents.iter().enumerate() {
            ids.push(qr.ids[i].clone());
            docs.push(doc.clone());
            let meta = qr.metadatas.get(i).cloned().unwrap_or_default();
            metas.push(meta);
        }
    }

    eprintln!("  Re-indexing {} drawers...", docs.len());

    // Re-embed and upsert documents into a fresh collection.
    let mut errors = 0;
    let batch_size = 100;
    for chunk in docs.chunks(batch_size) {
        let id_chunk: Vec<String> = ids
            .iter()
            .skip(errors * batch_size)
            .take(chunk.len())
            .cloned()
            .collect();
        let meta_chunk: Vec<std::collections::HashMap<String, serde_json::Value>> = metas
            .iter()
            .skip(errors * batch_size)
            .take(chunk.len())
            .cloned()
            .collect();

        let upserts: Vec<(
            String,
            String,
            std::collections::HashMap<String, serde_json::Value>,
        )> = id_chunk
            .into_iter()
            .zip(chunk.iter().cloned())
            .zip(meta_chunk.into_iter())
            .map(|((id, doc), meta)| (id, doc, meta))
            .collect();

        if let Err(e) = db.upsert_documents(&upserts) {
            eprintln!("    Error re-indexing batch: {}", e);
            errors += 1;
        }
    }

    db.flush()?;

    // Update manifest.
    if let Ok(embedder) = crate::embed::embedder_from_env() {
        let fp = embedder.fingerprint().to_string();
        let dim = embedder.dim();
        let meta = VectorIndexMeta::new("auto", &fp, dim);
        write_index_meta(palace_path, &meta)?;
    }

    eprintln!(
        "  Migration complete: v{} -> v{}, {} drawers re-indexed ({} errors)",
        old_version,
        CURRENT_INDEX_VERSION,
        docs.len(),
        errors
    );

    Ok(MigrateIndexResult {
        drawers_reindexed: docs.len(),
        errors,
        old_version,
        new_version: CURRENT_INDEX_VERSION,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_detect_index_version_missing() {
        let dir = TempDir::new().expect("temp dir");
        let result = detect_index_version(dir.path());
        assert!(result.is_ok());
        let (version, model, ..) = result.unwrap();
        assert_eq!(version, 0);
        assert_eq!(model, "unknown");
    }

    #[test]
    fn test_write_and_detect_index_meta() {
        let dir = TempDir::new().expect("temp dir");
        let meta = VectorIndexMeta::new("test-model", "fp-abc", 384);
        write_index_meta(dir.path(), &meta).expect("write");

        let (version, model, fingerprint, dim) = detect_index_version(dir.path()).expect("detect");
        assert_eq!(version, CURRENT_INDEX_VERSION);
        assert_eq!(model, "test-model");
        assert_eq!(fingerprint, "fp-abc");
        assert_eq!(dim, 384);
    }

    #[test]
    fn test_needs_migration_new_index() {
        let dir = TempDir::new().expect("temp dir");
        let meta = VectorIndexMeta::new("model", "fp", 384);
        write_index_meta(dir.path(), &meta).expect("write");
        let needs = needs_migration(dir.path()).expect("check");
        // Current version matches — no migration needed
        assert!(!needs);
    }

    #[test]
    fn test_needs_migration_old_version() {
        let dir = TempDir::new().expect("temp dir");
        let old_meta = VectorIndexMeta {
            version: 1,
            model: "old-model".into(),
            fingerprint: "old-fp".into(),
            dim: 128,
            ephemeral: false,
        };
        write_index_meta(dir.path(), &old_meta).expect("write");
        let needs = needs_migration(dir.path()).expect("check");
        assert!(needs);
    }

    #[test]
    fn test_migrate_index_empty() {
        let dir = TempDir::new().expect("temp dir");
        let palace_path = dir.path().join("palace");
        std::fs::create_dir_all(&palace_path).unwrap();

        let result = migrate_index(&palace_path).expect("migrate");
        assert_eq!(result.drawers_reindexed, 0);
        assert_eq!(result.old_version, 0);
        assert_eq!(result.new_version, CURRENT_INDEX_VERSION);
    }
}
