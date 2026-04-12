//! Dataset loader for LongMemEval benchmark data.
//!
//! Downloads from HuggingFace and parses the JSON format used by
//! https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// HuggingFace dataset identifier for LongMemEval cleaned data.
pub const LOMEMEVAL_DATASET: &str = "xiaowu0162/longmemeval-cleaned";
pub const LOMEMEVAL_FILE: &str = "longmemeval_s_cleaned.json";

/// A single turn in a conversation session.
#[derive(Debug, Clone, Deserialize)]
pub struct Turn {
    pub role: String,
    pub content: String,
}

/// A single benchmark entry (question + haystack).
#[derive(Debug, Clone, Deserialize)]
pub struct BenchmarkEntry {
    pub question_id: String,
    pub question: String,
    #[serde(rename = "question_type")]
    pub question_type: String,
    #[serde(rename = "question_date", default)]
    pub question_date: Option<String>,
    pub answer: serde_json::Value,
    #[serde(rename = "answer_session_ids")]
    pub answer_session_ids: Vec<String>,
    #[serde(rename = "haystack_session_ids")]
    pub haystack_session_ids: Vec<String>,
    #[serde(rename = "haystack_dates")]
    pub haystack_dates: Vec<String>,
    #[serde(rename = "haystack_sessions")]
    pub haystack_sessions: Vec<Vec<Turn>>,
}

/// Granularity for corpus construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Granularity {
    /// One document per session (all user turns joined).
    Session,
    /// One document per user turn.
    Turn,
}

/// Load dataset from local JSON file.
pub fn load_from_file(path: &Path) -> Result<Vec<BenchmarkEntry>> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("Failed to read {:?}", path))?;
    let entries: Vec<BenchmarkEntry> = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse JSON from {:?}", path))?;
    Ok(entries)
}

/// Download dataset from HuggingFace to local cache.
///
/// Returns the path to the downloaded file.
pub async fn download_from_huggingface(cache_dir: &Path) -> Result<PathBuf> {
    let cache_dir = cache_dir.to_path_buf();

    let file_path = cache_dir.join(LOMEMEVAL_FILE);

    // Check if already downloaded
    if file_path.exists() {
        return Ok(file_path);
    }

    // Build HuggingFace raw URL
    let url = format!(
        "https://huggingface.co/datasets/{}/resolve/main/{}",
        LOMEMEVAL_DATASET, LOMEMEVAL_FILE
    );

    println!("Downloading {} ...", url);

    // Use reqwest async (needs runtime with async features)
    let response = reqwest::get(&url)
        .await
        .context("Failed to send HTTP request to HuggingFace")?;

    if !response.status().is_success() {
        anyhow::bail!(
            "HuggingFace download failed with status {}: {}",
            response.status(),
            url
        );
    }

    let bytes = response
        .bytes()
        .await
        .context("Failed to read response bytes")?;

    // Use spawn_blocking for filesystem operations
    let file_path_clone = file_path.clone();
    let cache_dir_clone = cache_dir.clone();
    tokio::task::spawn_blocking(move || {
        use std::io::Write;

        std::fs::create_dir_all(&cache_dir_clone).context("Failed to create cache directory")?;

        let mut file =
            std::fs::File::create(&file_path_clone).context("Failed to create local file")?;
        file.write_all(&bytes)
            .context("Failed to write downloaded data")?;
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("Filesystem task failed")??;

    println!("Cached at {:?}", file_path);

    Ok(file_path)
}

/// Build a session-level corpus from a benchmark entry.
///
/// Returns (documents, corpus_ids) — one document per session.
/// Document text = all user turns joined with newlines.
pub fn build_session_corpus(entry: &BenchmarkEntry) -> (Vec<String>, Vec<String>) {
    let mut documents = Vec::new();
    let mut corpus_ids = Vec::new();

    for (i, session) in entry.haystack_sessions.iter().enumerate() {
        let session_id = entry
            .haystack_session_ids
            .get(i)
            .cloned()
            .unwrap_or_else(|| format!("sess_{:03}", i));

        // Join all USER turns only (matching Python behavior)
        let user_content: Vec<&str> = session
            .iter()
            .filter(|turn| turn.role == "user")
            .map(|turn| turn.content.as_str())
            .collect();

        let document = user_content.join("\n");

        if !document.is_empty() {
            documents.push(document);
            corpus_ids.push(session_id);
        }
    }

    (documents, corpus_ids)
}

/// Build a turn-level corpus from a benchmark entry.
///
/// Returns (documents, corpus_ids) — one document per user turn.
pub fn build_turn_corpus(entry: &BenchmarkEntry) -> (Vec<String>, Vec<String>) {
    let mut documents = Vec::new();
    let mut corpus_ids = Vec::new();

    for (i, session) in entry.haystack_sessions.iter().enumerate() {
        let session_id = entry
            .haystack_session_ids
            .get(i)
            .cloned()
            .unwrap_or_else(|| format!("sess_{:03}", i));

        for (turn_idx, turn) in session.iter().enumerate().filter(|(_, t)| t.role == "user") {
            let corpus_id = format!("{}_turn_{}", session_id, turn_idx);
            documents.push(turn.content.clone());
            corpus_ids.push(corpus_id);
        }
    }

    (documents, corpus_ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry() -> BenchmarkEntry {
        let haystack_sessions = vec![
            vec![
                Turn {
                    role: "user".to_string(),
                    content: "I worked on the auth migration today".to_string(),
                },
                Turn {
                    role: "assistant".to_string(),
                    content: "How did it go?".to_string(),
                },
            ],
            vec![Turn {
                role: "user".to_string(),
                content: "I still remember the happy high school experiences".to_string(),
            }],
        ];

        BenchmarkEntry {
            question_id: "test-001".to_string(),
            question: "high school reunion".to_string(),
            question_type: "single-session-preference".to_string(),
            question_date: Some("2023/08/15 (Tue) 14:30".to_string()),
            answer: serde_json::json!("You were in debate team."),
            answer_session_ids: vec!["sess_001".to_string()],
            haystack_session_ids: vec!["sess_000".to_string(), "sess_001".to_string()],
            haystack_dates: vec![
                "2023/01/10 (Tue) 09:00".to_string(),
                "2023/06/22 (Thu) 16:45".to_string(),
            ],
            haystack_sessions,
        }
    }

    #[test]
    fn test_build_session_corpus() {
        let entry = sample_entry();
        let (docs, ids) = build_session_corpus(&entry);

        assert_eq!(docs.len(), 2);
        assert_eq!(ids, vec!["sess_000", "sess_001"]);
        assert_eq!(docs[0], "I worked on the auth migration today");
        assert_eq!(
            docs[1],
            "I still remember the happy high school experiences"
        );
    }

    #[test]
    fn test_build_session_corpus_skips_assistant() {
        let entry = sample_entry();
        let (docs, _) = build_session_corpus(&entry);

        // First session has assistant turn but we only take user turns
        assert!(docs[0].contains("auth migration"));
        assert!(!docs[0].contains("How did it go"));
    }

    #[test]
    fn test_build_turn_corpus() {
        let entry = sample_entry();
        let (docs, ids) = build_turn_corpus(&entry);

        // 1 user turn in first session + 1 in second = 2
        assert_eq!(docs.len(), 2);
        assert_eq!(ids[0], "sess_000_turn_0");
        assert_eq!(ids[1], "sess_001_turn_0");
    }

    #[test]
    fn test_load_from_file_not_found() {
        let result = load_from_file(Path::new("/nonexistent/file.json"));
        assert!(result.is_err());
    }
}
