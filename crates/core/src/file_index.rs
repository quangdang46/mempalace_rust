//! File index — port of upstream `file-index.ts`.
//!
//! Tracks file history across sessions.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// History entry for a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileHistoryEntry {
    pub session_id: String,
    pub timestamp: DateTime<Utc>,
    pub action: String,
    pub observation_id: Option<String>,
}

/// Complete history for a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileHistory {
    pub path: String,
    pub entries: Vec<FileHistoryEntry>,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub session_count: usize,
}

/// Store for file index data.
pub struct FileIndex {
    entries: Vec<FileIndexEntry>,
}

#[derive(Debug, Clone)]
struct FileIndexEntry {
    file_path: String,
    session_id: String,
    timestamp: DateTime<Utc>,
    action: String,
    observation_id: Option<String>,
}

impl FileIndex {
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }

    pub fn record(&mut self, file_path: &str, session_id: &str, action: &str, observation_id: Option<&str>) {
        self.entries.push(FileIndexEntry {
            file_path: file_path.to_string(),
            session_id: session_id.to_string(),
            timestamp: Utc::now(),
            action: action.to_string(),
            observation_id: observation_id.map(String::from),
        });
    }

    pub fn history_for_file(&self, file_path: &str) -> FileHistory {
        let matching: Vec<_> = self.entries.iter()
            .filter(|e| e.file_path == file_path)
            .collect();

        if matching.is_empty() {
            return FileHistory {
                path: file_path.to_string(),
                entries: Vec::new(),
                first_seen: Utc::now(),
                last_seen: Utc::now(),
                session_count: 0,
            };
        }

        let entries: Vec<_> = matching.iter().map(|e| FileHistoryEntry {
            session_id: e.session_id.clone(),
            timestamp: e.timestamp,
            action: e.action.clone(),
            observation_id: e.observation_id.clone(),
        }).collect();

        let first_seen = entries.iter().map(|e| e.timestamp).min().unwrap_or(Utc::now());
        let last_seen = entries.iter().map(|e| e.timestamp).max().unwrap_or(Utc::now());
        let session_count = entries.iter()
            .map(|e| &e.session_id)
            .collect::<std::collections::HashSet<_>>()
            .len();

        FileHistory {
            path: file_path.to_string(),
            entries,
            first_seen,
            last_seen,
            session_count,
        }
    }

    pub fn files_in_session(&self, session_id: &str) -> Vec<String> {
        self.entries.iter()
            .filter(|e| e.session_id == session_id)
            .map(|e| e.file_path.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn sessions_for_file(&self, file_path: &str) -> Vec<String> {
        self.entries.iter()
            .filter(|e| e.file_path == file_path)
            .map(|e| e.session_id.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn most_active_files(&self, limit: usize) -> Vec<(String, usize)> {
        let mut counts = std::collections::HashMap::new();
        for entry in &self.entries {
            *counts.entry(entry.file_path.clone()).or_insert(0) += 1;
        }
        let mut sorted: Vec<_> = counts.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        sorted.truncate(limit);
        sorted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_and_history() {
        let mut index = FileIndex::new();
        index.record("src/main.rs", "s-1", "edit", Some("o-1"));
        index.record("src/main.rs", "s-2", "read", Some("o-2"));

        let history = index.history_for_file("src/main.rs");
        assert_eq!(history.path, "src/main.rs");
        assert_eq!(history.entries.len(), 2);
        assert_eq!(history.session_count, 2);
    }

    #[test]
    fn test_files_in_session() {
        let mut index = FileIndex::new();
        index.record("src/main.rs", "s-1", "edit", None);
        index.record("src/lib.rs", "s-1", "edit", None);
        index.record("src/main.rs", "s-2", "read", None);

        let files = index.files_in_session("s-1");
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_sessions_for_file() {
        let mut index = FileIndex::new();
        index.record("src/main.rs", "s-1", "edit", None);
        index.record("src/main.rs", "s-2", "edit", None);
        index.record("src/main.rs", "s-1", "read", None);

        let sessions = index.sessions_for_file("src/main.rs");
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn test_most_active_files() {
        let mut index = FileIndex::new();
        for _ in 0..5 { index.record("src/main.rs", "s-1", "edit", None); }
        for _ in 0..3 { index.record("src/lib.rs", "s-1", "edit", None); }
        for _ in 0..1 { index.record("src/util.rs", "s-1", "edit", None); }

        let top = index.most_active_files(2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].0, "src/main.rs");
        assert_eq!(top[0].1, 5);
    }

    #[test]
    fn test_empty_file_history() {
        let index = FileIndex::new();
        let history = index.history_for_file("nonexistent.rs");
        assert!(history.entries.is_empty());
        assert_eq!(history.session_count, 0);
    }

    #[test]
    fn test_file_history_serialization() {
        let entry = FileHistoryEntry {
            session_id: "s-1".to_string(),
            timestamp: Utc::now(),
            action: "edit".to_string(),
            observation_id: Some("o-1".to_string()),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: FileHistoryEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.session_id, "s-1");
        assert_eq!(parsed.action, "edit");
    }
}
