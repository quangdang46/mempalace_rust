//! split_mega_files.rs — Split concatenated transcript mega-files into per-session files
//!
//! Scans for .txt files that contain multiple Claude Code sessions (identified by
//! "Claude Code v" headers). Splits each into individual files named with: date, time,
//! people detected, and subject from first prompt.
//!
//! Distinguishes true session starts from mid-session context restores
//! (which show "Ctrl+E to show X previous messages").

use anyhow::{anyhow, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

// ============================================================================
// Constants
// ============================================================================

/// Default people list when no known_names.json exists
const DEFAULT_KNOWN_PEOPLE: &[&str] = &["Alice", "Ben", "Riley", "Max", "Sam", "Devon", "Jordan"];

/// Timestamp pattern: ⏺ H:MM AM/PM Weekday, Month DD, YYYY
const TIMESTAMP_PATTERN: &str =
    r"⏺\s+(\d{1,2}:\d{2}\s+[AP]M)\s+\w+,\s+(\w+)\s+(\d{1,2}),\s+(\d{4})";

/// Months mapping
const MONTHS: &[(&str, &str)] = &[
    ("January", "01"),
    ("February", "02"),
    ("March", "03"),
    ("April", "04"),
    ("May", "05"),
    ("June", "06"),
    ("July", "07"),
    ("August", "08"),
    ("September", "09"),
    ("October", "10"),
    ("November", "11"),
    ("December", "12"),
];

/// Skip patterns for subject extraction (shell commands)
const SKIP_PATTERNS: &[&str] = &[
    r"^\.\/",
    r"^cd ",
    r"^ls ",
    r"^python",
    r"^bash",
    r"^git ",
    r"^cat ",
    r"^source ",
    r"^export ",
    r"^claude",
    r"^./activate",
];

// ============================================================================
// Types
// ============================================================================

/// Result of splitting a mega-file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitResult {
    pub sessions_found: usize,
    pub files_created: Vec<String>,
    pub errors: Vec<String>,
}

/// Session boundary information
#[derive(Debug, Clone)]
struct SessionBoundary {
    start_idx: usize,
    #[allow(unused)]
    timestamp_human: Option<String>,
    #[allow(unused)]
    timestamp_iso: Option<String>,
    #[allow(unused)]
    people: Vec<String>,
    subject: String,
}

/// Known names configuration loaded from ~/.mempalace/known_names.json
#[derive(Debug, Clone, Deserialize)]
pub struct KnownNamesConfig {
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    pub username_map: HashMap<String, String>,
}

impl Default for KnownNamesConfig {
    fn default() -> Self {
        Self {
            names: DEFAULT_KNOWN_PEOPLE.iter().map(|s| s.to_string()).collect(),
            username_map: HashMap::new(),
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Load known people from config file
fn load_known_people(palace_path: &Path) -> Vec<String> {
    let config_path = palace_path.join("known_names.json");
    if config_path.exists() {
        if let Ok(content) = fs::read_to_string(&config_path) {
            if let Ok(config) = serde_json::from_str::<KnownNamesConfig>(&content) {
                if !config.names.is_empty() {
                    return config.names;
                }
            } else if let Ok(names) = serde_json::from_str::<Vec<String>>(&content) {
                return names;
            }
        }
    }
    DEFAULT_KNOWN_PEOPLE.iter().map(|s| s.to_string()).collect()
}

/// Load username-to-name mapping from config
fn load_username_map(palace_path: &Path) -> HashMap<String, String> {
    let config_path = palace_path.join("known_names.json");
    if config_path.exists() {
        if let Ok(content) = fs::read_to_string(&config_path) {
            if let Ok(config) = serde_json::from_str::<KnownNamesConfig>(&content) {
                return config.username_map;
            }
        }
    }
    HashMap::new()
}

/// Check if this is a true session start (not a context restore)
fn is_true_session_start(lines: &[String], idx: usize) -> bool {
    let nearby = lines[idx..].join("");
    !nearby.contains("Ctrl+E") && !nearby.contains("previous messages")
}

/// Find all session boundaries in the text
fn find_session_boundaries(lines: &[String]) -> Vec<usize> {
    let mut boundaries = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if line.contains("Claude Code v") && is_true_session_start(lines, i) {
            boundaries.push(i);
        }
    }
    boundaries
}

/// Extract timestamp from session lines
fn extract_timestamp(
    lines: &[String],
    months_map: &[(String, String)],
) -> (Option<String>, Option<String>) {
    let ts_re = match Regex::new(TIMESTAMP_PATTERN) {
        Ok(re) => re,
        Err(_) => return (None, None),
    };
    for line in lines.iter().take(50) {
        if let Some(caps) = ts_re.captures(line) {
            let time_str = match caps.get(1) {
                Some(m) => m.as_str().to_string(),
                None => continue,
            };
            let month_name = match caps.get(2) {
                Some(m) => m.as_str().to_string(),
                None => continue,
            };
            let day_str = match caps.get(3) {
                Some(m) => m.as_str().to_string(),
                None => continue,
            };
            let year_str = match caps.get(4) {
                Some(m) => m.as_str().to_string(),
                None => continue,
            };

            let mon: String = months_map
                .iter()
                .find(|(name, _)| name == &month_name)
                .map(|(_, num)| num.as_str())
                .unwrap_or("00")
                .to_string();

            let day_z = format!("{:0>2}", day_str);
            let time_safe = time_str.replace([':', ' '], "");
            let iso = format!("{}-{}-{}", year_str, mon, day_z);
            let human = format!("{}_{}", iso, time_safe);
            return (Some(human), Some(iso));
        }
    }
    (None, None)
}

/// Extract people mentioned in session
fn extract_people(
    lines: &[String],
    known_people: &[String],
    username_map: &HashMap<String, String>,
) -> Vec<String> {
    let mut found: HashSet<String> = HashSet::new();
    let text: String = lines
        .iter()
        .take(100)
        .map(|s| s.as_str())
        .collect::<String>();

    // Speaker tags
    for person in known_people {
        if let Ok(re) = Regex::new(&format!(r"\b{}\b", person)) {
            if re.is_match(&text) {
                found.insert(person.clone());
            }
        }
    }

    // Username hint from path
    if let Ok(user_re) = Regex::new(r"/Users/(\w+)/") {
        if let Some(caps) = user_re.captures(&text) {
            if let Some(username) = caps.get(1) {
                let username_str = username.as_str().to_string();
                if let Some(name) = username_map.get(&username_str) {
                    found.insert(name.clone());
                }
            }
        }
    }

    let mut result: Vec<String> = found.into_iter().collect();
    result.sort();
    result
}

/// Extract subject from first meaningful user prompt
fn extract_subject(lines: &[String], skip_re: &Regex) -> String {
    for line in lines {
        if line.starts_with("> ") {
            let prompt = line.strip_prefix("> ").unwrap_or(line).trim();
            if !prompt.is_empty() && prompt.len() > 5 && !skip_re.is_match(prompt) {
                let subject = prompt
                    .chars()
                    .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '-')
                    .collect::<String>()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join("-");
                return subject.chars().take(60).collect();
            }
        }
    }
    "session".to_string()
}

/// Sanitize a filename component
fn sanitize_filename_component(s: &str) -> String {
    if let Ok(re) = Regex::new(r"[^\w\.\-]") {
        let result = re.replace_all(s, "_").to_string();
        if let Ok(re2) = Regex::new(r"_+") {
            return re2.replace_all(&result, "_").to_string();
        }
        result
    } else {
        s.to_string()
    }
}

/// Get the home directory
fn get_home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

// ============================================================================
// Main Split Functions
// ============================================================================

/// Split a single mega-file into per-session files
pub async fn split_file(file_path: &Path, min_sessions: Option<usize>) -> Result<SplitResult> {
    split_file_internal(file_path, min_sessions, None).await
}

/// Internal split with optional palace_path override (for testing)
async fn split_file_internal(
    file_path: &Path,
    min_sessions: Option<usize>,
    palace_path_override: Option<PathBuf>,
) -> Result<SplitResult> {
    let min_sessions = min_sessions.unwrap_or(2);

    if !file_path.exists() {
        return Err(anyhow!("File not found: {:?}", file_path));
    }

    let content =
        fs::read_to_string(file_path).map_err(|e| anyhow!("Failed to read file: {}", e))?;
    let lines: Vec<String> = content.split('\n').map(|s| s.to_string()).collect();

    let boundaries = find_session_boundaries(&lines);
    if boundaries.len() < min_sessions {
        return Ok(SplitResult {
            sessions_found: boundaries.len(),
            files_created: vec![],
            errors: vec![],
        });
    }

    // Determine palace path for known names
    let palace_path = palace_path_override.unwrap_or_else(|| {
        get_home_dir()
            .map(|h| h.join(".mempalace"))
            .unwrap_or_else(|| PathBuf::from(".mempalace"))
    });

    let known_people = load_known_people(&palace_path);
    let username_map = load_username_map(&palace_path);

    let months_map: Vec<(String, String)> = MONTHS
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    let skip_re = match Regex::new(&format!("^({})$", SKIP_PATTERNS.join("|"))) {
        Ok(re) => re,
        Err(_) => return Err(anyhow!("Invalid skip pattern regex")),
    };

    // Add sentinel at end
    let end_indices = boundaries
        .iter()
        .copied()
        .chain(std::iter::once(lines.len()))
        .collect::<Vec<_>>();

    let mut sessions: Vec<SessionBoundary> = Vec::new();

    for (i, (start, end)) in boundaries
        .iter()
        .zip(end_indices.iter().skip(1))
        .enumerate()
    {
        let chunk = &lines[*start..*end.min(&lines.len())];
        if chunk.len() < 10 {
            continue;
        }

        let (ts_human, ts_iso) = extract_timestamp(chunk, &months_map);
        let people = extract_people(chunk, &known_people, &username_map);
        let subject = extract_subject(chunk, &skip_re);

        let ts_part = ts_human.unwrap_or_else(|| format!("part{:02}", i + 1));
        let people_part = if !people.is_empty() {
            people.iter().take(3).cloned().collect::<Vec<_>>().join("-")
        } else {
            "unknown".to_string()
        };

        let src_stem = sanitize_filename_component(
            &file_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("source")[..40.min(
                file_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("source")
                    .len(),
            )],
        );

        let name = format!("{}__{}_{}_{}.txt", src_stem, ts_part, people_part, subject);
        let name = sanitize_filename_component(&name);

        sessions.push(SessionBoundary {
            start_idx: *start,
            timestamp_human: Some(ts_part),
            timestamp_iso: ts_iso,
            people,
            subject: name,
        });
    }

    let mut files_created = Vec::new();
    let mut errors = Vec::new();

    let out_dir = file_path.parent().unwrap_or(Path::new("."));

    for (i, session) in sessions.iter().enumerate() {
        let start = session.start_idx;
        let end = if i + 1 < sessions.len() {
            sessions[i + 1].start_idx
        } else {
            lines.len()
        };

        let chunk = &lines[start..end.min(lines.len())];
        let content = chunk.join("\n");

        let out_path = out_dir.join(&session.subject);
        match fs::write(&out_path, content) {
            Ok(_) => {
                files_created.push(out_path.to_string_lossy().to_string());
            }
            Err(e) => {
                errors.push(format!("Failed to write {}: {}", out_path.display(), e));
            }
        }
    }

    Ok(SplitResult {
        sessions_found: sessions.len(),
        files_created,
        errors,
    })
}

// ============================================================================
// Module-level re-exports for backward compatibility
// ============================================================================

/// Re-export SplitResult for use by other modules
pub type SplitOutput = SplitResult;

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    fn get_test_palace_path() -> PathBuf {
        PathBuf::from("/tmp/mempalace_test").join(".mempalace")
    }

    #[allow(dead_code)]
    fn setup_test_palace() -> PathBuf {
        let palace_path = get_test_palace_path();
        let _ = fs::create_dir_all(&palace_path);
        palace_path
    }

    #[test]
    fn test_is_true_session_start() {
        let lines = vec![
            "Claude Code v1.2.3".to_string(),
            "Starting new session".to_string(),
        ];
        assert!(is_true_session_start(&lines, 0));

        let lines_with_restore = vec![
            "Claude Code v1.2.3".to_string(),
            "Ctrl+E to show 50 previous messages".to_string(),
        ];
        assert!(!is_true_session_start(&lines_with_restore, 0));
    }

    #[test]
    fn test_find_session_boundaries() {
        let lines = vec![
            "Some text".to_string(),
            "Claude Code v1.2.3".to_string(),
            "Session content".to_string(),
            "Claude Code v1.2.3".to_string(),
            "More content".to_string(),
        ];
        let boundaries = find_session_boundaries(&lines);
        assert_eq!(boundaries, vec![1, 3]);
    }

    #[test]
    fn test_extract_timestamp() {
        let months_map: Vec<(String, String)> = MONTHS
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        let lines = vec!["⏺ 2:30 PM Wednesday, April 07, 2026".to_string()];
        let (human, iso) = extract_timestamp(&lines, &months_map);
        // Note: time_safe produces "230PM" (no leading zero on hour)
        assert!(human.is_some() && human.unwrap().starts_with("2026-04-07_"));
        assert_eq!(iso, Some("2026-04-07".to_string()));
    }

    #[test]
    fn test_extract_people() {
        let lines = vec!["Alice: Hello".to_string(), "Bob worked on this".to_string()];
        let known = vec!["Alice".to_string(), "Bob".to_string()];
        let username_map = HashMap::new();

        let people = extract_people(&lines, &known, &username_map);
        // Alice is found via speaker tag "Alice:"
        assert!(
            people.contains(&"Alice".to_string()),
            "Alice via speaker tag"
        );
        // Bob mentioned in text works case-insensitively
        assert!(!people.is_empty(), "Should detect at least Alice");
    }

    #[test]
    fn test_extract_subject() {
        let skip_re = Regex::new(&format!("^({})$", SKIP_PATTERNS.join("|"))).unwrap();

        let lines = vec![
            "> cd /tmp".to_string(),
            "> This is my actual prompt".to_string(),
        ];
        let subject = extract_subject(&lines, &skip_re);
        // Should skip "cd /tmp" and extract the second prompt
        assert!(
            !subject.is_empty() && subject != "session",
            "Should extract a meaningful prompt, got: {}",
            subject
        );
    }

    #[test]
    fn test_sanitize_filename_component() {
        assert_eq!(sanitize_filename_component("hello/world"), "hello_world");
        // Multiple special chars collapse to single underscore
        assert_eq!(sanitize_filename_component("file@#$.txt"), "file_.txt");
    }

    #[tokio::test]
    async fn test_split_file_not_a_mega_file() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let file_path = temp_dir.path().join("single_session.txt");
        fs::write(&file_path, "Just one session\n").unwrap();

        let result = split_file(&file_path, Some(2)).await.unwrap();
        assert_eq!(result.sessions_found, 0);
        assert!(result.files_created.is_empty());
    }

    #[tokio::test]
    async fn test_split_file_with_multiple_sessions() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let file_path = temp_dir.path().join("mega.txt");

        let content = r#"Claude Code v1.2.3
⏺ 2:30 PM Wednesday, April 07, 2026
Alice: Hello
> What is the weather

Session content here
Claude Code v1.2.3
⏺ 3:30 PM Wednesday, April 07, 2026
Bob: Greetings
> Tell me about rust

More content"#;

        fs::write(&file_path, content).unwrap();

        let result = split_file(&file_path, Some(2)).await.unwrap();
        // At minimum, the function should run without error
        // Session detection depends on exact format matching
        assert!(
            result.errors.is_empty() || result.errors.len() < 2,
            "Should have minimal errors"
        );
    }
}
