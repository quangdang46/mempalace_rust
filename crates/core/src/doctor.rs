//! doctor.rs — Palace health check command
//!
//! Runs 7 independent checks to diagnose palace health:
//!   1. Palace directory exists and is accessible
//!   2. Config validity
//!   3. Drawer count and wing/room breakdown
//!   4. Orphan drawers (metadata references non-existent source files)
//!   5. Duplicate content detection
//!   6. Knowledge graph connectivity
//!   7. Embedder identity (active embedder vs `embedding.json` manifest)
//!
//! See [`run_doctor_with_options`] for the embedder check (mp-019). The
//! original [`run_doctor`] entry point preserves backward compatibility
//! and runs the embedder check in "manifest-only" mode (no live
//! embedder, no network).

use crate::embed::{Embedder, EmbeddingManifest};
use crate::knowledge_graph::KnowledgeGraph;
use crate::palace_db::PalaceDb;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug)]
#[non_exhaustive]
pub struct DoctorReport {
    pub checks: Vec<CheckResult>,
    pub healthy: bool,
}

#[derive(Debug)]
#[non_exhaustive]
pub struct CheckResult {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

pub fn run_doctor(palace_path: &Path) -> anyhow::Result<DoctorReport> {
    // Backward-compatible default: no live embedder, manifest-only
    // embedder check (acts as if `--no-network` was passed). Callers
    // that want the active-embedder comparison should use
    // `run_doctor_with_options`.
    run_doctor_with_options(palace_path, None, true)
}

/// Extended doctor entry point that can compare a live embedder
/// against the palace's `embedding.json` manifest (mp-019).
///
/// * `embedder` — borrowed reference to the active embedder. Pass
///   `None` if the caller hasn't loaded one (e.g. `mpr doctor` on a
///   machine without a downloaded model).
/// * `no_network` — when `true`, never inspect the live embedder even
///   if one is provided; report the manifest only. This is the escape
///   hatch for environments where instantiating the configured
///   embedder would trigger a model download.
pub fn run_doctor_with_options(
    palace_path: &Path,
    embedder: Option<&dyn Embedder>,
    no_network: bool,
) -> anyhow::Result<DoctorReport> {
    let mut checks = Vec::new();
    let palace_path = PathBuf::from(palace_path);

    // Check 1: Palace directory exists
    checks.push(check_palace_directory(&palace_path));

    // Check 2: Config validity
    checks.push(check_config_validity(&palace_path));

    // Check 3: Drawer inventory
    checks.push(check_drawer_inventory(&palace_path)?);

    // Check 4: Orphan drawers
    checks.push(check_orphan_drawers(&palace_path)?);

    // Check 5: Duplicate content
    checks.push(check_duplicate_drawers(&palace_path)?);

    // Check 6: Knowledge graph
    checks.push(check_knowledge_graph(&palace_path)?);

    // Check 7: Embedder identity (mp-019)
    checks.push(check_embedder(&palace_path, embedder, no_network));

    let healthy = !checks.iter().any(|c| c.status == CheckStatus::Fail);

    Ok(DoctorReport { checks, healthy })
}

fn check_palace_directory(palace_path: &Path) -> CheckResult {
    if palace_path.exists() && palace_path.is_dir() {
        CheckResult {
            name: "palace_directory".to_string(),
            status: CheckStatus::Pass,
            message: format!("Palace directory exists: {}", palace_path.display()),
        }
    } else if !palace_path.exists() {
        CheckResult {
            name: "palace_directory".to_string(),
            status: CheckStatus::Warn,
            message: "Palace directory does not exist yet. Run `mpr init` to create it."
                .to_string(),
        }
    } else {
        CheckResult {
            name: "palace_directory".to_string(),
            status: CheckStatus::Fail,
            message: format!(
                "Palace path exists but is not a directory: {}",
                palace_path.display()
            ),
        }
    }
}

fn check_config_validity(palace_path: &Path) -> CheckResult {
    let config_path = palace_path.join("config.json");
    if !config_path.exists() {
        return CheckResult {
            name: "config_validity".to_string(),
            status: CheckStatus::Warn,
            message: "config.json not found (will use defaults)".to_string(),
        };
    }

    match std::fs::read_to_string(&config_path) {
        Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(_) => CheckResult {
                name: "config_validity".to_string(),
                status: CheckStatus::Pass,
                message: "config.json is valid".to_string(),
            },
            Err(e) => CheckResult {
                name: "config_validity".to_string(),
                status: CheckStatus::Fail,
                message: format!("config.json is invalid JSON: {}", e),
            },
        },
        Err(e) => CheckResult {
            name: "config_validity".to_string(),
            status: CheckStatus::Fail,
            message: format!("Cannot read config.json: {}", e),
        },
    }
}

fn check_drawer_inventory(palace_path: &Path) -> anyhow::Result<CheckResult> {
    let db = PalaceDb::open(palace_path)?;
    let count = db.count();

    if count == 0 {
        return Ok(CheckResult {
            name: "drawer_inventory".to_string(),
            status: CheckStatus::Warn,
            message: "No drawers found. Run `mpr mine <dir>` to start filling the palace."
                .to_string(),
        });
    }

    let results = db.get_all(None, None, 1000);
    let mut wing_counts: HashMap<String, usize> = HashMap::new();

    for r in &results {
        if let Some(w) = r
            .metadatas
            .first()
            .and_then(|m| m.get("wing"))
            .and_then(|v| v.as_str())
        {
            *wing_counts.entry(w.to_string()).or_insert(0) += 1;
        }
    }

    let wing_list: Vec<String> = wing_counts
        .iter()
        .map(|(w, n)| format!("{} ({})", w, n))
        .collect();
    let message = format!(
        "{} drawers across {} wings: {}",
        count,
        wing_counts.len(),
        wing_list.join(", ")
    );

    Ok(CheckResult {
        name: "drawer_inventory".to_string(),
        status: CheckStatus::Pass,
        message,
    })
}

fn check_orphan_drawers(palace_path: &Path) -> anyhow::Result<CheckResult> {
    let db = PalaceDb::open(palace_path)?;
    let results = db.get_all(None, None, 1000);

    let mut orphans = Vec::new();
    for r in &results {
        if let Some(source) = r
            .metadatas
            .first()
            .and_then(|m| m.get("source_file"))
            .and_then(|v| v.as_str())
        {
            if !source.is_empty() && !Path::new(source).exists() {
                orphans.push(source.to_string());
            }
        }
    }

    if orphans.is_empty() {
        Ok(CheckResult {
            name: "orphan_drawers".to_string(),
            status: CheckStatus::Pass,
            message: "All drawer source files are present".to_string(),
        })
    } else {
        let msg = if orphans.len() > 5 {
            format!(
                "{} orphan drawers (source files missing): {}",
                orphans.len(),
                orphans[..5].join(", ")
            )
        } else {
            format!("{} orphan drawers: {}", orphans.len(), orphans.join(", "))
        };
        Ok(CheckResult {
            name: "orphan_drawers".to_string(),
            status: CheckStatus::Warn,
            message: msg,
        })
    }
}

fn check_duplicate_drawers(palace_path: &Path) -> anyhow::Result<CheckResult> {
    let db = PalaceDb::open(palace_path)?;
    let results = db.get_all(None, None, 1000);

    let mut content_set: HashSet<String> = HashSet::new();
    let mut duplicates: Vec<String> = Vec::new();

    for r in &results {
        let content = r.documents.first().cloned().unwrap_or_default();
        let short = if content.len() > 100 {
            &content[..100]
        } else {
            &content
        };
        let key = short.to_string();
        if content_set.contains(&key) {
            duplicates.push(short.to_string());
        } else {
            content_set.insert(key);
        }
    }

    if duplicates.is_empty() {
        Ok(CheckResult {
            name: "duplicate_drawers".to_string(),
            status: CheckStatus::Pass,
            message: "No duplicate content detected".to_string(),
        })
    } else {
        Ok(CheckResult {
            name: "duplicate_drawers".to_string(),
            status: CheckStatus::Warn,
            message: format!("{} potentially duplicate drawers found", duplicates.len()),
        })
    }
}

fn check_knowledge_graph(palace_path: &Path) -> anyhow::Result<CheckResult> {
    let kg_path = palace_path.join("knowledge_graph.db");
    if !kg_path.exists() {
        return Ok(CheckResult {
            name: "knowledge_graph".to_string(),
            status: CheckStatus::Warn,
            message: "Knowledge graph not initialized yet".to_string(),
        });
    }

    let kg = KnowledgeGraph::open(&kg_path)?;
    let stats = kg.stats()?;

    if stats.total_triples == 0 {
        return Ok(CheckResult {
            name: "knowledge_graph".to_string(),
            status: CheckStatus::Warn,
            message: "Knowledge graph is empty (no facts recorded yet)".to_string(),
        });
    }

    Ok(CheckResult {
        name: "knowledge_graph".to_string(),
        status: CheckStatus::Pass,
        message: format!(
            "{} entities, {} triples ({} current, {} expired)",
            stats.total_entities, stats.total_triples, stats.current_facts, stats.expired_facts
        ),
    })
}

/// Check 7 — embedder identity (mp-019).
///
/// Reports the active embedder's fingerprint, dim, and source
/// (env-var override vs default), reads the palace's `embedding.json`
/// manifest if present, and compares them.
///
/// Outcomes:
///   * ✅ `Pass`  — manifest matches the active embedder.
///   * ⚠️ `Warn` — manifest absent (will be written on next write),
///                 manifest present but no active embedder, or
///                 `--no-network` mode without a manifest.
///   * ❌ `Fail` — manifest disagrees with the active embedder; the
///                 message tells the user to run
///                 `mpr migrate --re-embed`.
///
/// `--no-network` (`no_network = true`) skips inspection of the live
/// embedder so a `doctor` run on a fresh machine can still surface
/// what the palace recorded without triggering a model download.
fn check_embedder(
    palace_path: &Path,
    embedder: Option<&dyn Embedder>,
    no_network: bool,
) -> CheckResult {
    let manifest_path = EmbeddingManifest::path(palace_path);
    let manifest = match EmbeddingManifest::read(palace_path) {
        Ok(m) => m,
        Err(e) => {
            return CheckResult {
                name: "embedder".to_string(),
                status: CheckStatus::Fail,
                message: format!(
                    "Failed to read embedding manifest at {}: {}",
                    manifest_path.display(),
                    e
                ),
            };
        }
    };

    // Source: env-var override vs default. Reported alongside the
    // active embedder so users can tell at a glance whether their
    // palace is running on the configured model or an override.
    let env_model = std::env::var("MEMPALACE_EMBED_MODEL").ok();
    let source = match &env_model {
        Some(m) => format!("env-var override (MEMPALACE_EMBED_MODEL={m})"),
        None => "default".to_string(),
    };

    // --no-network: never touch the live embedder, even if one was
    // provided. Report what the manifest records, or warn that the
    // palace has no recorded identity yet.
    if no_network {
        return match manifest {
            Some(m) => CheckResult {
                name: "embedder".to_string(),
                status: CheckStatus::Pass,
                message: format!(
                    "Embedder check skipped (--no-network); manifest: model={} fingerprint={} (dim {}) created_at={} (source: {})",
                    m.model_name,
                    m.fingerprint,
                    m.dim,
                    m.created_at.to_rfc3339(),
                    source,
                ),
            },
            None => CheckResult {
                name: "embedder".to_string(),
                status: CheckStatus::Warn,
                message: format!(
                    "Embedder check skipped (--no-network); no manifest found at {} (source: {})",
                    manifest_path.display(),
                    source,
                ),
            },
        };
    }

    // No live embedder injected. We still surface the manifest so
    // operators can see what the palace recorded, but we can't make
    // the active-vs-recorded comparison without one.
    let Some(active) = embedder else {
        return match manifest {
            Some(m) => CheckResult {
                name: "embedder".to_string(),
                status: CheckStatus::Warn,
                message: format!(
                    "No active embedder configured; manifest: model={} fingerprint={} (dim {}) created_at={} (source: {})",
                    m.model_name,
                    m.fingerprint,
                    m.dim,
                    m.created_at.to_rfc3339(),
                    source,
                ),
            },
            None => CheckResult {
                name: "embedder".to_string(),
                status: CheckStatus::Warn,
                message: format!(
                    "No active embedder configured and no manifest found at {} (source: {})",
                    manifest_path.display(),
                    source,
                ),
            },
        };
    };

    let active_fp = active.fingerprint().to_string();
    let active_dim = active.dim();

    match manifest {
        Some(m) => {
            if m.fingerprint == active_fp && m.dim == active_dim {
                CheckResult {
                    name: "embedder".to_string(),
                    status: CheckStatus::Pass,
                    message: format!(
                        "✅ Embedder: {} (dim {}) — manifest matches (source: {}, created_at: {})",
                        active_fp,
                        active_dim,
                        source,
                        m.created_at.to_rfc3339(),
                    ),
                }
            } else {
                CheckResult {
                    name: "embedder".to_string(),
                    status: CheckStatus::Fail,
                    message: format!(
                        "❌ Embedder mismatch: palace recorded {} (dim {}), active is {} (dim {}). Run `mpr migrate --re-embed`.",
                        m.fingerprint,
                        m.dim,
                        active_fp,
                        active_dim,
                    ),
                }
            }
        }
        None => CheckResult {
            name: "embedder".to_string(),
            status: CheckStatus::Warn,
            message: format!(
                "⚠️ Embedder: {} (dim {}) — manifest absent (will be written on next write). Source: {}",
                active_fp, active_dim, source,
            ),
        },
    }
}

impl std::fmt::Display for CheckStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckStatus::Pass => write!(f, "PASS"),
            CheckStatus::Warn => write!(f, "WARN"),
            CheckStatus::Fail => write!(f, "FAIL"),
        }
    }
}

impl std::fmt::Display for CheckResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}: {}", self.status, self.name, self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::NullEmbedder;
    use tempfile::TempDir;

    #[test]
    fn test_check_status_display() {
        assert_eq!(CheckStatus::Pass.to_string(), "PASS");
        assert_eq!(CheckStatus::Warn.to_string(), "WARN");
        assert_eq!(CheckStatus::Fail.to_string(), "FAIL");
    }

    #[test]
    fn test_run_doctor_empty() {
        let temp_dir = TempDir::new().unwrap();
        let result = run_doctor(temp_dir.path()).unwrap();
        // 7 checks now (palace dir, config, drawers, orphans, dupes, kg, embedder)
        assert!(result.checks.len() >= 7);
        // The embedder check is the last one and should warn on a
        // bare temp dir (no manifest, no embedder).
        let last = result.checks.last().expect("at least one check");
        assert_eq!(last.name, "embedder");
    }

    /// mp-019: with a live embedder and no manifest, the check warns
    /// that the manifest is absent and reports the active embedder's
    /// fingerprint and dim verbatim.
    #[test]
    fn test_check_embedder_manifest_absent() {
        let temp_dir = TempDir::new().unwrap();
        let embedder = NullEmbedder::new(384);
        let result = check_embedder(temp_dir.path(), Some(&embedder), false);
        assert_eq!(result.status, CheckStatus::Warn);
        assert!(
            result.message.contains("Embedder: null:384 (dim 384)"),
            "got: {}",
            result.message
        );
        assert!(
            result.message.contains("manifest absent"),
            "got: {}",
            result.message
        );
        assert!(
            result.message.contains("written on next write"),
            "got: {}",
            result.message
        );
    }

    /// mp-019: with a manifest matching the live embedder, the check
    /// passes with the canonical "manifest matches" message.
    #[test]
    fn test_check_embedder_manifest_matches() {
        let temp_dir = TempDir::new().unwrap();
        let embedder = NullEmbedder::new(384);
        let manifest = EmbeddingManifest::from_embedder(&embedder, "null-test");
        EmbeddingManifest::write(temp_dir.path(), &manifest).unwrap();

        let result = check_embedder(temp_dir.path(), Some(&embedder), false);
        assert_eq!(
            result.status,
            CheckStatus::Pass,
            "expected Pass, got {}: {}",
            result.status,
            result.message
        );
        assert!(
            result.message.contains("Embedder: null:384 (dim 384)"),
            "got: {}",
            result.message
        );
        assert!(
            result.message.contains("manifest matches"),
            "got: {}",
            result.message
        );
    }

    /// mp-019: a fingerprint disagreement between manifest and active
    /// embedder is a Fail with an actionable message.
    #[test]
    fn test_check_embedder_fingerprint_mismatch() {
        let temp_dir = TempDir::new().unwrap();

        // Hand-write a manifest with a different fingerprint than
        // NullEmbedder(384) would produce. We use the public schema
        // shape directly so the test doesn't depend on the
        // hypothetical FixedEmbedder helper.
        let recorded = EmbeddingManifest::from_embedder(
            &NullEmbedder::new(384), // dim matches
            "null-test",
        );
        // Replace fingerprint after construction by writing JSON.
        let mut json = serde_json::to_value(&recorded).unwrap();
        json["fingerprint"] = serde_json::Value::String("fastembed:bge-small-en-v15:384".into());
        std::fs::write(
            EmbeddingManifest::path(temp_dir.path()),
            serde_json::to_vec_pretty(&json).unwrap(),
        )
        .unwrap();

        let active = NullEmbedder::new(384);
        let result = check_embedder(temp_dir.path(), Some(&active), false);
        assert_eq!(
            result.status,
            CheckStatus::Fail,
            "expected Fail, got {}: {}",
            result.status,
            result.message
        );
        assert!(
            result.message.contains("Embedder mismatch"),
            "got: {}",
            result.message
        );
        assert!(
            result
                .message
                .contains("palace recorded fastembed:bge-small-en-v15:384"),
            "got: {}",
            result.message
        );
        assert!(
            result.message.contains("active is null:384"),
            "got: {}",
            result.message
        );
        assert!(
            result.message.contains("mpr migrate --re-embed"),
            "got: {}",
            result.message
        );
    }

    /// mp-019: a dimension disagreement is a Fail; both dims appear
    /// in the message and the recovery command is suggested.
    #[test]
    fn test_check_embedder_dim_mismatch() {
        let temp_dir = TempDir::new().unwrap();
        // Manifest written from a 768-dim embedder.
        let recorded = EmbeddingManifest::from_embedder(&NullEmbedder::new(768), "null-test");
        EmbeddingManifest::write(temp_dir.path(), &recorded).unwrap();

        let active = NullEmbedder::new(384);
        let result = check_embedder(temp_dir.path(), Some(&active), false);
        assert_eq!(
            result.status,
            CheckStatus::Fail,
            "expected Fail, got {}: {}",
            result.status,
            result.message
        );
        assert!(
            result.message.contains("(dim 768)"),
            "got: {}",
            result.message
        );
        assert!(
            result.message.contains("(dim 384)"),
            "got: {}",
            result.message
        );
        assert!(
            result.message.contains("mpr migrate --re-embed"),
            "got: {}",
            result.message
        );
    }

    /// mp-019: `--no-network` reports the manifest only and never
    /// inspects the embedder, so the message says
    /// "Embedder check skipped".
    #[test]
    fn test_check_embedder_no_network_with_manifest() {
        let temp_dir = TempDir::new().unwrap();
        let embedder = NullEmbedder::new(384);
        let manifest = EmbeddingManifest::from_embedder(&embedder, "null-test");
        EmbeddingManifest::write(temp_dir.path(), &manifest).unwrap();

        let result = check_embedder(temp_dir.path(), None, true);
        assert_eq!(result.status, CheckStatus::Pass);
        assert!(
            result.message.contains("--no-network"),
            "got: {}",
            result.message
        );
        assert!(
            result.message.contains("fingerprint=null:384"),
            "got: {}",
            result.message
        );
        assert!(
            result.message.contains("model=null-test"),
            "got: {}",
            result.message
        );
    }

    /// mp-019: `--no-network` with no manifest warns that nothing is
    /// recorded, instead of trying to load an embedder.
    #[test]
    fn test_check_embedder_no_network_no_manifest() {
        let temp_dir = TempDir::new().unwrap();
        let result = check_embedder(temp_dir.path(), None, true);
        assert_eq!(result.status, CheckStatus::Warn);
        assert!(
            result.message.contains("--no-network"),
            "got: {}",
            result.message
        );
        assert!(
            result.message.contains("no manifest found"),
            "got: {}",
            result.message
        );
    }

    /// mp-019: with no embedder and no manifest (and no `--no-network`),
    /// we still warn rather than fail — operators may simply not have
    /// loaded an embedder yet.
    #[test]
    fn test_check_embedder_no_embedder_no_manifest() {
        let temp_dir = TempDir::new().unwrap();
        let result = check_embedder(temp_dir.path(), None, false);
        assert_eq!(result.status, CheckStatus::Warn);
        assert!(
            result.message.contains("No active embedder configured"),
            "got: {}",
            result.message
        );
        assert!(
            result.message.contains("no manifest found"),
            "got: {}",
            result.message
        );
    }

    /// mp-019: with no embedder but a manifest, surface the recorded
    /// identity so operators can audit a palace without loading the
    /// model.
    #[test]
    fn test_check_embedder_no_embedder_with_manifest() {
        let temp_dir = TempDir::new().unwrap();
        let embedder = NullEmbedder::new(384);
        let manifest = EmbeddingManifest::from_embedder(&embedder, "null-test");
        EmbeddingManifest::write(temp_dir.path(), &manifest).unwrap();

        let result = check_embedder(temp_dir.path(), None, false);
        assert_eq!(result.status, CheckStatus::Warn);
        assert!(
            result.message.contains("No active embedder configured"),
            "got: {}",
            result.message
        );
        assert!(
            result.message.contains("fingerprint=null:384"),
            "got: {}",
            result.message
        );
    }

    /// mp-019: `run_doctor_with_options` runs all 7 checks and the
    /// embedder check uses the live embedder when one is supplied.
    #[test]
    fn test_run_doctor_with_options_includes_embedder() {
        let temp_dir = TempDir::new().unwrap();
        let embedder = NullEmbedder::new(384);
        let manifest = EmbeddingManifest::from_embedder(&embedder, "null-test");
        EmbeddingManifest::write(temp_dir.path(), &manifest).unwrap();

        let report = run_doctor_with_options(temp_dir.path(), Some(&embedder), false).unwrap();
        assert!(report.checks.len() >= 7);
        let last = report.checks.last().expect("at least one check");
        assert_eq!(last.name, "embedder");
        assert_eq!(last.status, CheckStatus::Pass);
        assert!(
            last.message.contains("manifest matches"),
            "got: {}",
            last.message
        );
    }
}
