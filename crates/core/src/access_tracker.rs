//! Memory access tracking — port of upstream `access-tracker.ts`.
//!
//! Tracks how often memories are accessed, when they were last accessed,
//! and provides recent access history.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Access log entry for a single memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessLog {
    pub memory_id: String,
    pub count: usize,
    pub last_at: DateTime<Utc>,
    pub recent: Vec<DateTime<Utc>>,
}

impl AccessLog {
    pub fn new(memory_id: String) -> Self {
        Self {
            memory_id,
            count: 0,
            last_at: Utc::now(),
            recent: Vec::new(),
        }
    }

    pub fn record_access(&mut self, max_recent: usize) {
        let now = Utc::now();
        self.count += 1;
        self.last_at = now;
        self.recent.push(now);
        if self.recent.len() > max_recent {
            self.recent.remove(0);
        }
    }
}

/// Store for access tracking data.
pub struct AccessTracker {
    logs: std::collections::HashMap<String, AccessLog>,
    max_recent: usize,
}

impl AccessTracker {
    pub fn new(max_recent: usize) -> Self {
        Self {
            logs: std::collections::HashMap::new(),
            max_recent,
        }
    }

    pub fn record(&mut self, memory_id: &str) {
        let log = self.logs
            .entry(memory_id.to_string())
            .or_insert_with(|| AccessLog::new(memory_id.to_string()));
        log.record_access(self.max_recent);
    }

    pub fn get(&self, memory_id: &str) -> Option<&AccessLog> {
        self.logs.get(memory_id)
    }

    pub fn count(&self, memory_id: &str) -> usize {
        self.logs.get(memory_id).map(|l| l.count).unwrap_or(0)
    }

    pub fn last_at(&self, memory_id: &str) -> Option<DateTime<Utc>> {
        self.logs.get(memory_id).map(|l| l.last_at)
    }

    pub fn recent_accesses(&self, memory_id: &str) -> Vec<DateTime<Utc>> {
        self.logs.get(memory_id)
            .map(|l| l.recent.clone())
            .unwrap_or_default()
    }

    pub fn most_accessed(&self, limit: usize) -> Vec<&AccessLog> {
        let mut logs: Vec<&AccessLog> = self.logs.values().collect();
        logs.sort_by(|a, b| b.count.cmp(&a.count));
        logs.truncate(limit);
        logs
    }

    pub fn recently_accessed(&self, limit: usize) -> Vec<&AccessLog> {
        let mut logs: Vec<&AccessLog> = self.logs.values().collect();
        logs.sort_by(|a, b| b.last_at.cmp(&a.last_at));
        logs.truncate(limit);
        logs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_access() {
        let mut tracker = AccessTracker::new(5);
        tracker.record("m-1");
        tracker.record("m-1");
        assert_eq!(tracker.count("m-1"), 2);
    }

    #[test]
    fn test_recent_keeps_limit() {
        let mut tracker = AccessTracker::new(3);
        for _ in 0..5 {
            tracker.record("m-1");
        }
        let log = tracker.get("m-1").unwrap();
        assert_eq!(log.recent.len(), 3);
        assert_eq!(log.count, 5);
    }

    #[test]
    fn test_most_accessed() {
        let mut tracker = AccessTracker::new(5);
        for _ in 0..10 { tracker.record("m-1"); }
        for _ in 0..5 { tracker.record("m-2"); }
        for _ in 0..1 { tracker.record("m-3"); }
        let top = tracker.most_accessed(2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].memory_id, "m-1");
        assert_eq!(top[1].memory_id, "m-2");
    }

    #[test]
    fn test_last_at() {
        let mut tracker = AccessTracker::new(5);
        assert!(tracker.last_at("m-1").is_none());
        tracker.record("m-1");
        assert!(tracker.last_at("m-1").is_some());
    }

    #[test]
    fn test_recently_accessed() {
        let mut tracker = AccessTracker::new(5);
        tracker.record("m-1");
        std::thread::sleep(std::time::Duration::from_millis(10));
        tracker.record("m-2");
        let recent = tracker.recently_accessed(2);
        assert_eq!(recent[0].memory_id, "m-2");
    }

    #[test]
    fn test_empty_tracker() {
        let tracker = AccessTracker::new(5);
        assert!(tracker.get("nonexistent").is_none());
        assert_eq!(tracker.count("nonexistent"), 0);
        assert!(tracker.most_accessed(10).is_empty());
    }
}
