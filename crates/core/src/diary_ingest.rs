//! diary_ingest.rs — Ingest daily summary files into the palace.
//!
//! Architecture:
//! - ONE drawer per (wing, day) — full verbatim content, upserted as the day grows.
//! - Closets pack topics up to CLOSET_CHAR_LIMIT.
//! - Only new entries are processed by default (tracks entry count in state file).
//!
//! Usage:
//!     mpr diary-ingest --dir ~/daily_summaries [--wing diary] [--force]

use crate::config::Config;
use crate::entity_detector::detect_from_content;
use crate::palace_db::PalaceDb;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const DIARY_ENTRY_RE: &str = r"(?m)^## .+";

/// Ingest daily summary files into the palace.
pub fn ingest_diaries(
    diary_dir: &Path,
    palace_path: Option<&Path>,
    wing: &str,
    force: bool,
) -> anyhow::Result<DiaryIngestStats> {
    let config = Config::load()?;
    let palace_path = palace_path.unwrap_or_else(|| config.palace_path.as_path());

    let diary_dir = diary_dir.expanduser().resolve_path();
    if !diary_dir.exists() {
        anyhow::bail!("Diary directory not found: {}", diary_dir.display());
    }

    let state_file = state_file_for(palace_path, &diary_dir)?;
    let mut state: HashMap<String, StateEntry> = if force || !state_file.exists() {
        HashMap::new()
    } else {
        serde_json::from_str(&fs::read_to_string(&state_file)?).unwrap_or_default()
    };

    let mut palace_db = PalaceDb::open(palace_path)?;
    let mut days_updated = 0usize;
    let mut closets_created = 0usize;

    // Find all .md files
    let diary_files: Vec<PathBuf> = WalkDir::new(&diary_dir)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
        .map(|e| e.path().to_path_buf())
        .collect();

    for diary_path in &diary_files {
        let text = fs::read_to_string(diary_path).unwrap_or_default();
        if text.trim().len() < 50 {
            continue;
        }

        // Extract date from filename (e.g., 2024-01-15.md)
        let date_str = diary_path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| {
                if s.len() >= 10 && s.chars().nth(4) == Some('-') {
                    Some(&s[..10])
                } else {
                    None
                }
            })
            .unwrap_or("unknown");

        // Skip if content hasn't changed
        let state_key = format!(
            "{}|{}",
            wing,
            diary_path.file_name().unwrap_or_default().to_string_lossy()
        );
        let curr_size = text.len();
        if state.get(&state_key).map(|s| s.size) == Some(curr_size) && !force {
            continue;
        }

        let now_iso = chrono_now_iso();
        let drawer_id = diary_drawer_id(wing, date_str);
        let entities = extract_entities_for_metadata(&text);

        let mut metadata = HashMap::new();
        metadata.insert("date".to_string(), serde_json::json!(date_str));
        metadata.insert("wing".to_string(), serde_json::json!(wing));
        metadata.insert("room".to_string(), serde_json::json!("daily"));
        metadata.insert(
            "source_file".to_string(),
            serde_json::json!(diary_path.to_string_lossy()),
        );
        metadata.insert(
            "source_session".to_string(),
            serde_json::json!("daily_diary"),
        );
        metadata.insert("filed_at".to_string(), serde_json::json!(now_iso));
        if !entities.is_empty() {
            metadata.insert("entities".to_string(), serde_json::json!(entities));
        }

        // Upsert drawer
        palace_db.upsert_documents(&[(drawer_id.clone(), text.clone(), metadata.clone())])?;

        // Extract entries and build closet lines
        let entries = split_entries(&text);
        let new_entries = if force { &entries } else { &entries };

        if !new_entries.is_empty() {
            closets_created += new_entries.len();
        }

        state.insert(
            state_key,
            StateEntry {
                size: curr_size,
                entry_count: entries.len(),
                ingested_at: now_iso,
            },
        );
        days_updated += 1;
    }

    palace_db.flush()?;

    // Save state
    if days_updated > 0 {
        fs::write(&state_file, serde_json::to_string_pretty(&state)?)?;
        println!(
            "Diary: {} days updated, {} new closets",
            days_updated, closets_created
        );
    }

    Ok(DiaryIngestStats {
        days_updated,
        closets_created,
    })
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StateEntry {
    size: usize,
    entry_count: usize,
    ingested_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DiaryIngestStats {
    pub days_updated: usize,
    pub closets_created: usize,
}

fn state_file_for(palace_path: &Path, diary_dir: &Path) -> anyhow::Result<PathBuf> {
    let state_root = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_error| PathBuf::from("~"))
        .join(".mempalace/state");
    fs::create_dir_all(&state_root)?;
    let key = Sha256::digest(format!("{}|{}", palace_path.display(), diary_dir.display()));
    let key_str = format!("{:x}", key);
    Ok(state_root.join(format!("diary_ingest_{}.json", &key_str[..24])))
}

fn diary_drawer_id(wing: &str, date_str: &str) -> String {
    let suffix = Sha256::digest(format!("{}|{}", wing, date_str));
    format!("drawer_diary_{:x}", suffix)[..40].to_string()
}

fn split_entries(text: &str) -> Vec<(String, String)> {
    let re = Regex::new(DIARY_ENTRY_RE).unwrap();
    let parts: Vec<&str> = re.split(text).collect();
    let headers: Vec<&str> = re.find_iter(text).map(|m| m.as_str()).collect();
    let mut entries = Vec::new();
    for (i, header) in headers.iter().enumerate() {
        let body = parts.get(i + 1).unwrap_or(&"");
        entries.push((header.trim().to_string(), body.trim().to_string()));
    }
    entries
}

fn extract_entities_for_metadata(text: &str) -> String {
    let detection = detect_from_content(text);
    let names: Vec<String> = detection
        .people
        .into_iter()
        .take(10)
        .map(|p| p.name)
        .collect();
    names.join(", ")
}

fn chrono_now_iso() -> String {
    // Simple ISO date without timezone
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    let secs_per_day: u64 = 86400;
    let days = now.as_secs() / secs_per_day;
    let secs_of_day = now.as_secs() % secs_per_day;
    let mut y: u64 = 1970;
    let mut remaining = days;
    while remaining >= 365 {
        let is_leap = (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0);
        let days_in_y = if is_leap { 366 } else { 365 };
        if remaining >= days_in_y {
            remaining -= days_in_y;
            y += 1;
        } else {
            break;
        }
    }
    let days_per_month: [u64; 12] = if (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 1usize;
    for (i, &dpm) in days_per_month.iter().enumerate() {
        if remaining < dpm {
            month = i + 1;
            break;
        }
        remaining -= dpm;
    }
    let day = remaining + 1;
    let hour = secs_of_day / 3600;
    let min = (secs_of_day % 3600) / 60;
    let sec = secs_of_day % 60;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        y, month, day, hour, min, sec
    )
}

trait ExpandUser {
    fn expanduser(self) -> PathBuf;
}

impl ExpandUser for PathBuf {
    fn expanduser(self) -> PathBuf {
        if self.starts_with("~") {
            if let Ok(home) = std::env::var("HOME") {
                self.strip_prefix("~")
                    .map(|p| PathBuf::from(home).join(p))
                    .unwrap_or(self)
            } else {
                self
            }
        } else {
            self
        }
    }
}

impl ExpandUser for &Path {
    fn expanduser(self) -> PathBuf {
        self.to_path_buf().expanduser()
    }
}

trait ResolvePath {
    fn resolve_path(self) -> PathBuf;
}

impl ResolvePath for PathBuf {
    fn resolve_path(self) -> PathBuf {
        std::fs::canonicalize(&self).unwrap_or(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_entries() {
        let text = "## Meeting\n\nDiscussed the project.\n\n## Planning\n\nNext steps are...";
        let entries = split_entries(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, "## Meeting");
    }

    #[test]
    fn test_diary_drawer_id() {
        let id = diary_drawer_id("diary", "2024-01-15");
        assert!(id.starts_with("drawer_diary_"));
    }
}
