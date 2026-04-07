//! doctor.rs — Palace health check command
//!
//! Runs 6 independent checks to diagnose palace health:
//!   1. Palace directory exists and is accessible
//!   2. Config validity
//!   3. Drawer count and wing/room breakdown
//!   4. Orphan drawers (metadata references non-existent source files)
//!   5. Duplicate content detection
//!   6. Knowledge graph connectivity

use crate::knowledge_graph::KnowledgeGraph;
use crate::palace_db::PalaceDb;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct DoctorReport {
    pub checks: Vec<CheckResult>,
    pub healthy: bool,
}

#[derive(Debug)]
pub struct CheckResult {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

pub fn run_doctor(palace_path: &Path) -> anyhow::Result<DoctorReport> {
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
        if let Some(w) = r.metadatas.first().and_then(|m| m.get("wing")).and_then(|v| v.as_str())
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
        if let Some(source) = r.metadatas
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
        let short = if content.len() > 100 { &content[..100] } else { &content };
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
        assert!(result.checks.len() >= 6);
    }
}
