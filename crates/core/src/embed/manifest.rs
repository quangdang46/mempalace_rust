// =====================================================================
// `embedding.json` — palace embedding-model identity manifest
// (mp-015 / mp-016 / ADR-8)
// =====================================================================
//
// Each palace persists a single `embedding.json` sibling file alongside
// the collection JSON. Its job is to make embedder swaps fail loud
// instead of silently corrupting search results: cosine-similarity
// between a 384-dim BGE-Small vector and a 768-dim Qwen3-Embedding
// vector is meaningless, but the in-process HNSW index would happily
// accept either.
//
// On `Palace::open` (see `palace_db.rs`):
//   * If `embedding.json` is **missing** and we have an active embedder
//     to fingerprint, the manifest is written from the live embedder.
//     Legacy palaces (drawers exist, no manifest) get a best-effort
//     manifest written and a `tracing::warn!`.
//   * If **present**, we call [`EmbeddingManifest::validate_against`]
//     and surface a `ManifestMismatch` whose error message tells the
//     user the recovery action: `mpr migrate --re-embed`.
//
// `MEMPALACE_SKIP_MANIFEST_CHECK=1` skips validation. This is a
// deliberate test-and-migration backdoor only — production code paths
// must not set it.
//
// References:
//   - docs/research/00_UPGRADE_AND_INTEGRATION_PLAN.md ADR-8
//     "Embedding model identity & multi-tenant isolation"
//   - docs/research/05_embedding_and_storage_native.md §A.5
//   - mp-015 (this file), mp-016 (validation wired into `PalaceDb::open`)

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::Embedder;

/// Filename of the manifest, relative to the palace root.
///
/// Kept as a constant so callers don't accidentally typo `embeddings.json`
/// (the trailing `s` would silently make every palace look "legacy").
pub const MANIFEST_FILE_NAME: &str = "embedding.json";

/// Temporary file used by the atomic write-then-rename path in
/// [`EmbeddingManifest::write`]. A crash mid-write may leave this file
/// behind; [`EmbeddingManifest::read`] ignores it on the next open so
/// the palace is recoverable without manual cleanup.
pub const MANIFEST_TMP_FILE_NAME: &str = "embedding.json.tmp";

/// Persistent record of which embedder produced the vectors stored in
/// a palace. Written on first open, validated on every subsequent open.
///
/// Schema is intentionally tiny so the on-disk format can stay stable
/// while the surrounding code evolves. Add new fields with `serde`
/// defaults when needed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmbeddingManifest {
    /// Human-readable model name, e.g. `"bge-small-en-v15"` or
    /// `"BAAI/bge-small-en-v1.5"`. Not used for validation — that's
    /// `fingerprint`'s job — but included for `mpr doctor` output and
    /// error messages.
    pub model_name: String,
    /// Embedding dimensionality (384, 768, 1024, …). Validated on
    /// open: a mismatch is unrecoverable without re-embedding because
    /// the underlying HNSW index uses this dimension as a hard schema.
    pub dim: usize,
    /// Stable identifier returned by [`Embedder::fingerprint`].
    /// Conventionally `"<backend>:<model_id>:<dim>"`, e.g.
    /// `"fastembed:bge-small-en-v15:384"` or `"null:384"`.
    pub fingerprint: String,
    /// UTC timestamp the manifest was first written. Useful for
    /// forensics ("did this palace predate the upgrade?") but not
    /// validated on open.
    pub created_at: DateTime<Utc>,
    /// `mempalace-core` package version that wrote the manifest.
    /// Recorded for the same reason as `created_at` — diagnostic only.
    pub mempalace_version: String,
}

/// Why a manifest validation failed.
///
/// The variants intentionally carry the recorded vs runtime values
/// inline so the error message tells the user the exact mismatch and
/// the recovery command (`mpr migrate --re-embed`).
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ManifestMismatch {
    /// The embedder's `fingerprint()` does not match what the palace
    /// recorded on first write. Vectors produced by different
    /// fingerprints are not commensurable even when `dim` happens to
    /// match (e.g. BGE-Small vs E5-Small are both 384-dim).
    #[error(
        "embedding model fingerprint changed: palace recorded {recorded:?}, runtime is {runtime:?}. Run `mpr migrate --re-embed` to rebuild vectors."
    )]
    Fingerprint {
        /// Fingerprint persisted in `embedding.json`.
        recorded: String,
        /// Fingerprint reported by the live embedder.
        runtime: String,
    },
    /// The embedder's `dim()` does not match the recorded value. This
    /// is the harder failure mode: the in-process HNSW would refuse to
    /// add new vectors at the wrong dimension, so we fail loud at open
    /// instead of letting the caller hit a confusing storage error
    /// later.
    #[error(
        "embedding dimension changed: palace recorded {recorded}, runtime is {runtime}. Vectors are incompatible; run `mpr migrate --re-embed`."
    )]
    Dim {
        /// Dimension persisted in `embedding.json`.
        recorded: usize,
        /// Dimension reported by the live embedder.
        runtime: usize,
    },
}

impl EmbeddingManifest {
    /// Build a manifest from the live embedder. The `model_name`
    /// argument is supplied by the caller because the [`Embedder`]
    /// trait deliberately keeps the human-readable name out of its
    /// surface — different backends have different concepts of a
    /// "name" and we don't want to bake one in.
    pub fn from_embedder(embedder: &dyn Embedder, model_name: &str) -> Self {
        Self {
            model_name: model_name.to_string(),
            dim: embedder.dim(),
            fingerprint: embedder.fingerprint().to_string(),
            created_at: Utc::now(),
            mempalace_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// Path to the manifest file inside `palace_path`.
    pub fn path(palace_path: &Path) -> PathBuf {
        palace_path.join(MANIFEST_FILE_NAME)
    }

    /// Path to the atomic-write tempfile inside `palace_path`.
    pub fn tmp_path(palace_path: &Path) -> PathBuf {
        palace_path.join(MANIFEST_TMP_FILE_NAME)
    }

    /// Read the manifest from `palace_path/embedding.json`.
    ///
    /// Returns `Ok(None)` when the file is absent (legacy palaces and
    /// brand-new palaces both hit this path — the open code in
    /// `palace_db.rs` distinguishes them by checking drawer count).
    /// A leftover `embedding.json.tmp` from a crashed write is
    /// **ignored** on purpose: the rename in [`Self::write`] is the
    /// only commit point.
    pub fn read(palace_path: &Path) -> anyhow::Result<Option<Self>> {
        let path = Self::path(palace_path);
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path)?;
        let manifest: EmbeddingManifest = serde_json::from_slice(&bytes).map_err(|e| {
            anyhow::anyhow!(
                "failed to parse embedding manifest at {}: {}",
                path.display(),
                e
            )
        })?;
        Ok(Some(manifest))
    }

    /// Atomically write the manifest to `palace_path/embedding.json`.
    ///
    /// Implementation: write `embedding.json.tmp`, then `rename` it
    /// into place. POSIX `rename(2)` is atomic on the same filesystem;
    /// readers therefore either see the old manifest or the new one,
    /// never a half-written file. Windows' `MoveFileExW` is similarly
    /// atomic for files on the same volume.
    pub fn write(palace_path: &Path, manifest: &Self) -> anyhow::Result<()> {
        std::fs::create_dir_all(palace_path)?;
        let tmp = Self::tmp_path(palace_path);
        let final_path = Self::path(palace_path);
        let bytes = serde_json::to_vec_pretty(manifest)?;
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &final_path)?;
        Ok(())
    }

    /// Validate this manifest against a live embedder.
    ///
    /// Order matters: dim is checked **first** because a dim mismatch
    /// is the strictly-more-fatal failure (HNSW would reject new
    /// inserts), and we want the user to see the more actionable error
    /// when both happen to differ at once.
    pub fn validate_against(&self, embedder: &dyn Embedder) -> Result<(), ManifestMismatch> {
        if self.dim != embedder.dim() {
            return Err(ManifestMismatch::Dim {
                recorded: self.dim,
                runtime: embedder.dim(),
            });
        }
        if self.fingerprint != embedder.fingerprint() {
            return Err(ManifestMismatch::Fingerprint {
                recorded: self.fingerprint.clone(),
                runtime: embedder.fingerprint().to_string(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::NullEmbedder;

    /// A second null-style embedder with a custom fingerprint, used to
    /// drive the fingerprint-mismatch test without touching
    /// `NullEmbedder` (which is the production stub and intentionally
    /// has a fixed fingerprint format).
    struct FixedEmbedder {
        dim: usize,
        fingerprint: String,
    }

    #[async_trait::async_trait]
    impl Embedder for FixedEmbedder {
        fn dim(&self) -> usize {
            self.dim
        }
        fn fingerprint(&self) -> &str {
            &self.fingerprint
        }
        async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
            Ok(vec![0.0; self.dim])
        }
        async fn embed_batch(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
            Ok(vec![vec![0.0; self.dim]; texts.len()])
        }
    }

    /// mp-015: write + read roundtrip preserves every field.
    #[test]
    fn roundtrip_preserves_fields() {
        let temp = tempfile::tempdir().unwrap();
        let palace = temp.path();

        let embedder = NullEmbedder::new(384);
        let manifest = EmbeddingManifest::from_embedder(&embedder, "null-test");

        EmbeddingManifest::write(palace, &manifest).unwrap();
        let read_back = EmbeddingManifest::read(palace).unwrap().expect("present");

        assert_eq!(read_back, manifest);
        assert_eq!(read_back.model_name, "null-test");
        assert_eq!(read_back.dim, 384);
        assert_eq!(read_back.fingerprint, "null:384");
        assert_eq!(read_back.mempalace_version, env!("CARGO_PKG_VERSION"));
    }

    /// mp-015: a missing manifest returns `Ok(None)` so legacy palaces
    /// can keep opening.
    #[test]
    fn read_returns_none_when_missing() {
        let temp = tempfile::tempdir().unwrap();
        let read = EmbeddingManifest::read(temp.path()).unwrap();
        assert!(read.is_none());
    }

    /// mp-015: a leftover `.tmp` from a crashed write is ignored — only
    /// the renamed `embedding.json` is the commit point.
    #[test]
    fn crash_mid_write_leaves_tmp_but_read_returns_none() {
        let temp = tempfile::tempdir().unwrap();
        let palace = temp.path();

        // Simulate a crash *between* the `write` of `.tmp` and the
        // `rename` to `embedding.json`. We write the tmp file directly.
        let tmp = EmbeddingManifest::tmp_path(palace);
        std::fs::write(&tmp, b"{\"garbage\":true}").unwrap();
        assert!(tmp.is_file(), "tmp file should exist after simulated crash");

        // The committed file does not exist, so `read` must return None.
        let read = EmbeddingManifest::read(palace).unwrap();
        assert!(
            read.is_none(),
            "read must ignore the leftover .tmp; commit point is the rename"
        );
    }

    /// mp-016: identical embedder validates clean.
    #[test]
    fn validate_ok_when_embedder_matches() {
        let embedder = NullEmbedder::new(384);
        let manifest = EmbeddingManifest::from_embedder(&embedder, "null-test");
        assert!(manifest.validate_against(&embedder).is_ok());
    }

    /// mp-016: a dimension mismatch is reported as `ManifestMismatch::Dim`
    /// and the message tells the user how to recover.
    #[test]
    fn validate_err_dim_when_dim_changes() {
        let recorded_embedder = NullEmbedder::new(384);
        let manifest = EmbeddingManifest::from_embedder(&recorded_embedder, "null-test");

        let runtime_embedder = NullEmbedder::new(768);
        let err = manifest.validate_against(&runtime_embedder).unwrap_err();
        match &err {
            ManifestMismatch::Dim { recorded, runtime } => {
                assert_eq!(*recorded, 384);
                assert_eq!(*runtime, 768);
            }
            other => panic!("expected Dim mismatch, got {other:?}"),
        }
        let msg = err.to_string();
        assert!(
            msg.contains("mpr migrate --re-embed"),
            "error message must point at the recovery command: {msg}"
        );
    }

    /// mp-016: same dim but different fingerprint (e.g. swapping
    /// BGE-Small for E5-Small) is reported as `Fingerprint` and the
    /// message also points at `mpr migrate --re-embed`.
    #[test]
    fn validate_err_fingerprint_when_only_fingerprint_changes() {
        // Recorded: NullEmbedder(384) → fingerprint "null:384"
        let recorded_embedder = NullEmbedder::new(384);
        let manifest = EmbeddingManifest::from_embedder(&recorded_embedder, "null-test");

        // Runtime: same dim, different fingerprint.
        let runtime_embedder = FixedEmbedder {
            dim: 384,
            fingerprint: "fastembed:bge-small-en-v15:384".to_string(),
        };
        let err = manifest.validate_against(&runtime_embedder).unwrap_err();
        match &err {
            ManifestMismatch::Fingerprint { recorded, runtime } => {
                assert_eq!(recorded, "null:384");
                assert_eq!(runtime, "fastembed:bge-small-en-v15:384");
            }
            other => panic!("expected Fingerprint mismatch, got {other:?}"),
        }
        let msg = err.to_string();
        assert!(
            msg.contains("mpr migrate --re-embed"),
            "error message must point at the recovery command: {msg}"
        );
    }

    /// mp-016: when both dim and fingerprint differ, dim is reported
    /// first because it's the strictly-more-fatal mismatch.
    #[test]
    fn validate_err_dim_takes_precedence_over_fingerprint() {
        let recorded_embedder = NullEmbedder::new(384);
        let manifest = EmbeddingManifest::from_embedder(&recorded_embedder, "null-test");

        let runtime_embedder = FixedEmbedder {
            dim: 768,
            fingerprint: "totally-different-fingerprint".to_string(),
        };
        let err = manifest.validate_against(&runtime_embedder).unwrap_err();
        assert!(
            matches!(err, ManifestMismatch::Dim { .. }),
            "dim mismatch should be reported first, got {err:?}"
        );
    }

    /// mp-015: write atomically replaces an existing manifest. After
    /// two writes, the second value is what `read` returns.
    #[test]
    fn second_write_replaces_first() {
        let temp = tempfile::tempdir().unwrap();
        let palace = temp.path();

        let first = NullEmbedder::new(384);
        EmbeddingManifest::write(palace, &EmbeddingManifest::from_embedder(&first, "first"))
            .unwrap();

        let second = NullEmbedder::new(768);
        let m2 = EmbeddingManifest::from_embedder(&second, "second");
        EmbeddingManifest::write(palace, &m2).unwrap();

        let read_back = EmbeddingManifest::read(palace).unwrap().unwrap();
        assert_eq!(read_back.model_name, "second");
        assert_eq!(read_back.dim, 768);
        assert_eq!(read_back.fingerprint, "null:768");
    }

    /// mp-015: on disk shape is plain JSON we can re-parse without the
    /// strongly-typed struct, so future tooling (`mpr doctor`) can
    /// inspect manifests without linking the manifest module.
    #[test]
    fn on_disk_format_is_plain_json() {
        let temp = tempfile::tempdir().unwrap();
        let palace = temp.path();

        let embedder = NullEmbedder::new(384);
        let manifest = EmbeddingManifest::from_embedder(&embedder, "null-test");
        EmbeddingManifest::write(palace, &manifest).unwrap();

        let bytes = std::fs::read(EmbeddingManifest::path(palace)).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["model_name"], "null-test");
        assert_eq!(value["dim"], 384);
        assert_eq!(value["fingerprint"], "null:384");
        assert!(value["created_at"].is_string());
        assert_eq!(value["mempalace_version"], env!("CARGO_PKG_VERSION"));
    }
}
