use crate::types::AuditEntry;
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

pub struct AuditStore {
    entries: Vec<AuditEntry>,
}

impl AuditStore {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn record(
        &mut self,
        operation: &str,
        function_id: &str,
        target_ids: Vec<String>,
        details: HashMap<String, serde_json::Value>,
        quality_score: Option<f64>,
        user_id: Option<String>,
    ) -> AuditEntry {
        let entry = AuditEntry {
            id: format!("aud-{}", uuid::Uuid::new_v4()),
            timestamp: Utc::now(),
            operation: operation.to_string(),
            user_id,
            function_id: function_id.to_string(),
            target_ids,
            details,
            quality_score,
        };
        self.entries.push(entry.clone());
        entry
    }

    pub fn safe_record(
        &mut self,
        operation: &str,
        function_id: &str,
        target_ids: Vec<String>,
        details: HashMap<String, serde_json::Value>,
        quality_score: Option<f64>,
        user_id: Option<String>,
    ) {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.record(operation, function_id, target_ids, details, quality_score, user_id);
        })) {
            Ok(_) => {}
            Err(e) => {
                eprintln!("audit write failed: {:?}", e);
            }
        }
    }

    pub fn query(
        &self,
        operation: Option<&str>,
        date_from: Option<DateTime<Utc>>,
        date_to: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Vec<&AuditEntry> {
        let mut filtered: Vec<_> = self.entries.iter().collect();

        if let Some(op) = operation {
            filtered.retain(|e| e.operation == op);
        }

        if let Some(from) = date_from {
            filtered.retain(|e| e.timestamp >= from);
        }

        if let Some(to) = date_to {
            filtered.retain(|e| e.timestamp <= to);
        }

        filtered.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        filtered.into_iter().take(limit).collect()
    }

    pub fn entries(&self) -> &[AuditEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_details() -> HashMap<String, serde_json::Value> {
        let mut map = HashMap::new();
        map.insert("action".to_string(), serde_json::json!("test"));
        map
    }

    #[test]
    fn test_record_entry() {
        let mut store = AuditStore::new();
        let entry = store.record(
            "share",
            "mem::team-share",
            vec!["mem-1".to_string()],
            make_details(),
            None,
            Some("agent-1".to_string()),
        );

        assert_eq!(entry.operation, "share");
        assert_eq!(entry.function_id, "mem::team-share");
        assert_eq!(entry.target_ids, vec!["mem-1"]);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn test_safe_record_does_not_panic() {
        let mut store = AuditStore::new();
        store.safe_record(
            "test_op",
            "mem::test",
            vec!["id-1".to_string()],
            make_details(),
            None,
            None,
        );
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn test_query_by_operation() {
        let mut store = AuditStore::new();
        store.record("share", "mem::team-share", vec!["1".into()], make_details(), None, None);
        store.record("delete", "mem::forget", vec!["2".into()], make_details(), None, None);
        store.record("share", "mem::team-share", vec!["3".into()], make_details(), None, None);

        let shares = store.query(Some("share"), None, None, 100);
        assert_eq!(shares.len(), 2);

        let deletes = store.query(Some("delete"), None, None, 100);
        assert_eq!(deletes.len(), 1);
    }

    #[test]
    fn test_query_by_date_range() {
        let mut store = AuditStore::new();
        store.record("op1", "fn1", vec!["1".into()], make_details(), None, None);
        store.record("op2", "fn2", vec!["2".into()], make_details(), None, None);

        let now = Utc::now();
        let all = store.query(None, None, None, 100);
        assert_eq!(all.len(), 2);

        let after = store.query(None, Some(now - chrono::Duration::hours(1)), None, 100);
        assert_eq!(after.len(), 2);

        let before = store.query(None, None, Some(now - chrono::Duration::hours(1)), 100);
        assert_eq!(before.len(), 0);
    }

    #[test]
    fn test_query_respects_limit() {
        let mut store = AuditStore::new();
        for i in 0..10 {
            store.record(
                &format!("op-{}", i),
                &format!("fn-{}", i),
                vec![format!("id-{}", i)],
                make_details(),
                None,
                None,
            );
        }

        let limited = store.query(None, None, None, 3);
        assert_eq!(limited.len(), 3);
    }

    #[test]
    fn test_query_returns_newest_first() {
        let mut store = AuditStore::new();
        store.record("old", "fn1", vec!["1".into()], make_details(), None, None);
        std::thread::sleep(std::time::Duration::from_millis(10));
        store.record("new", "fn2", vec!["2".into()], make_details(), None, None);

        let results = store.query(None, None, None, 10);
        assert_eq!(results[0].operation, "new");
        assert_eq!(results[1].operation, "old");
    }

    #[test]
    fn test_record_with_quality_score() {
        let mut store = AuditStore::new();
        let entry = store.record(
            "evolve",
            "mem::evolve",
            vec!["mem-1".into()],
            make_details(),
            Some(0.85),
            None,
        );
        assert_eq!(entry.quality_score, Some(0.85));
    }

    #[test]
    fn test_empty_store() {
        let store = AuditStore::new();
        assert!(store.is_empty());
        assert_eq!(store.query(None, None, None, 100).len(), 0);
    }
}
