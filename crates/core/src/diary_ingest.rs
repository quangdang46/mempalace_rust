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
    let palace_path = palace_path.unwrap_or(config.palace_path.as_path());

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

        // Skip if content hasn't changed. Hash-based — size alone false-negatives
        // on same-length edits (e.g. "teh" → "the"), silently dropping real edits.
        let state_key = format!(
            "{}|{}",
            wing,
            diary_path.file_name().unwrap_or_default().to_string_lossy()
        );
        let curr_size = text.len();
        let curr_hash = format!("{:x}", Sha256::digest(text.as_bytes()));
        if !force {
            if let Some(prev) = state.get(&state_key) {
                if let Some(prev_hash) = prev.content_hash.as_ref() {
                    if prev_hash == &curr_hash {
                        continue;
                    }
                } else if prev.size > 0 && prev.size == curr_size {
                    // Legacy state without content_hash: keep size-based skip so a
                    // post-upgrade run doesn't re-ingest every untouched diary.
                    continue;
                }
            }
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
        let new_entries = &entries;

        if !new_entries.is_empty() {
            closets_created += new_entries.len();
        }

        state.insert(
            state_key,
            StateEntry {
                size: curr_size,
                content_hash: Some(curr_hash),
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
    /// sha256 hex digest of the diary file's text content. `None` is the
    /// legacy schema (size-only); kept optional so a post-upgrade run does
    /// not re-ingest every untouched diary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_hash: Option<String>,
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
    let detection = detect_from_content(text, None);
    let names: Vec<String> = detection
        .people
        .into_iter()
        .take(10)
        .map(|p| p.name)
        .collect();
    names.join(", ")
}

#[allow(clippy::manual_is_multiple_of)]
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

    #[test]
    fn test_state_entry_legacy_format_deserializes_without_content_hash() {
        // Regression for upstream mempalace 0d1c1fb: legacy state files
        // written before the content_hash field existed must still load.
        let legacy = r#"{"size": 42, "entry_count": 3, "ingested_at": "2024-01-01T00:00:00"}"#;
        let parsed: StateEntry = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.size, 42);
        assert_eq!(parsed.entry_count, 3);
        assert!(parsed.content_hash.is_none());
    }

    #[test]
    fn test_state_entry_round_trips_with_content_hash() {
        let entry = StateEntry {
            size: 10,
            content_hash: Some("deadbeef".to_string()),
            entry_count: 1,
            ingested_at: "2024-01-01T00:00:00".to_string(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("content_hash"));
        let parsed: StateEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.content_hash.as_deref(), Some("deadbeef"));
    }

    #[test]
    fn test_ingest_diaries_detects_same_size_edit() {
        // Regression for upstream mempalace 0d1c1fb (#925): an in-place edit
        // that preserves byte length (e.g. "teh" -> "the") was silently
        // dropped under the old size-only gate. Switch to content_hash.
        let _guard = crate::test_env_lock()
            .lock()
            .expect("test env lock should be available");
        let temp = tempfile::TempDir::new().unwrap();
        let prev_home = std::env::var_os("HOME");
        // diary_ingest::state_file_for uses $HOME/.mempalace/state — point it
        // at the tempdir so the test does not pollute real home.
        std::env::set_var("HOME", temp.path());

        let palace_path = temp.path().join("palace");
        let diary_dir = temp.path().join("diaries");
        std::fs::create_dir_all(&diary_dir).unwrap();

        // Write a diary file with enough content to clear the >50-byte filter.
        let diary_file = diary_dir.join("2024-01-15.md");
        let original = "## Notes\n\nteh quick brown fox jumps over the lazy dog and again here.\n";
        std::fs::write(&diary_file, original).unwrap();

        let stats = ingest_diaries(&diary_dir, Some(&palace_path), "diary", false).unwrap();
        assert_eq!(
            stats.days_updated, 1,
            "first ingest should record the diary"
        );

        // Second ingest with no change: must skip.
        let stats = ingest_diaries(&diary_dir, Some(&palace_path), "diary", false).unwrap();
        assert_eq!(
            stats.days_updated, 0,
            "second ingest with unchanged content should skip"
        );

        // Same-size edit: "teh" -> "the". Old gate would silently drop this.
        let edited = original.replace("teh", "the");
        assert_eq!(
            edited.len(),
            original.len(),
            "test fixture must preserve length"
        );
        std::fs::write(&diary_file, &edited).unwrap();

        let stats = ingest_diaries(&diary_dir, Some(&palace_path), "diary", false).unwrap();
        assert_eq!(
            stats.days_updated, 1,
            "same-size edit must be detected via content hash"
        );

        match prev_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }
}
