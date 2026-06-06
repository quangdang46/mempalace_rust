//! Event-sourced coordination log for deterministic replay and crash recovery.
//!
//! Records all coordination mutations as an append-only event log.
//! Enables crash recovery by replaying events, and supports compaction
//! to remove old events.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// All coordination mutations are recorded as events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event")]
pub enum CoordinationEvent {
    // Signal events
    SignalSent {
        signal_id: String,
        from: String,
        to: String,
        signal_type: String,
    },
    SignalRead {
        signal_id: String,
        agent_id: String,
    },

    // Lease events
    LeaseAcquired {
        lease_id: String,
        action_id: String,
        agent_id: String,
        ttl_minutes: i64,
    },
    LeaseReleased {
        lease_id: String,
        result: Option<String>,
    },
    LeaseRenewed {
        lease_id: String,
        extend_minutes: i64,
    },
    LeaseExpired {
        lease_id: String,
    },

    // Action events
    ActionCreated {
        action_id: String,
        title: String,
        status: String,
        priority: u8,
    },
    ActionStatusChanged {
        action_id: String,
        from: String,
        to: String,
    },
    ActionEdgeAdded {
        from_id: String,
        to_id: String,
        edge_type: String,
    },

    // Team events
    TeamItemShared {
        item_id: String,
        shared_by: String,
        item_type: String,
    },

    // File reservation events
    FileReserved {
        reservation_id: String,
        path: String,
        agent_id: String,
        mode: String,
    },
    FileReleased {
        reservation_id: String,
        path: String,
        agent_id: String,
    },

    // Live delivery events
    DeliveryQueued {
        signal_id: String,
        to_agent: String,
    },
    DeliveryInjected {
        signal_id: String,
        turn_number: usize,
    },
    DeliveryAcknowledged {
        signal_id: String,
    },
    DeliveryFailed {
        signal_id: String,
        reason: String,
    },
}

/// Snapshot of coordination state at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinationSnapshot {
    pub latest_sequence: u64,
    pub event_count: usize,
    pub created_at: DateTime<Utc>,
}

/// Event-sourced coordination log backed by SQLite.
pub struct CoordinationEventLog {
    conn: Connection,
}

impl CoordinationEventLog {
    /// Open or create the event log at the given path.
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;
        let log = Self { conn };
        log.init_db()?;
        Ok(log)
    }

    fn init_db(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS coordination_events (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                event_type TEXT NOT NULL,
                event_data TEXT NOT NULL,
                created_at TEXT NOT NULL,
                correlation_id TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_events_type ON coordination_events(event_type);
            CREATE INDEX IF NOT EXISTS idx_events_created ON coordination_events(created_at);
            CREATE INDEX IF NOT EXISTS idx_events_correlation ON coordination_events(correlation_id);
            ",
        )?;
        Ok(())
    }

    /// Append a single event to the log. Returns the sequence number.
    pub fn append(&self, event: &CoordinationEvent) -> Result<u64> {
        let event_type = Self::event_type_name(event);
        let event_data = serde_json::to_string(event)?;
        let created_at = Utc::now().to_rfc3339();

        self.conn.execute(
            "INSERT INTO coordination_events (event_type, event_data, created_at) VALUES (?1, ?2, ?3)",
            params![event_type, event_data, created_at],
        )?;

        let sequence = self.conn.last_insert_rowid() as u64;
        Ok(sequence)
    }

    /// Append a batch of events atomically. Returns the last sequence number.
    pub fn append_batch(&self, events: &[CoordinationEvent]) -> Result<u64> {
        let tx = self.conn.unchecked_transaction()?;
        let mut last_sequence = 0u64;

        for event in events {
            let event_type = Self::event_type_name(event);
            let event_data = serde_json::to_string(event)?;
            let created_at = Utc::now().to_rfc3339();

            tx.execute(
                "INSERT INTO coordination_events (event_type, event_data, created_at) VALUES (?1, ?2, ?3)",
                params![event_type, event_data, created_at],
            )?;

            last_sequence = tx.last_insert_rowid() as u64;
        }

        tx.commit()?;
        Ok(last_sequence)
    }

    /// Replay events starting from a sequence number.
    pub fn replay_from(&self, sequence: u64) -> Result<Vec<(u64, CoordinationEvent)>> {
        let mut stmt = self.conn.prepare(
            "SELECT sequence, event_data FROM coordination_events WHERE sequence >= ?1 ORDER BY sequence",
        )?;

        let rows = stmt.query_map(params![sequence], |row| {
            let seq: u64 = row.get(0)?;
            let data: String = row.get(1)?;
            Ok((seq, data))
        })?;

        let mut events = Vec::new();
        for row in rows {
            let (seq, data) = row?;
            if let Ok(event) = serde_json::from_str::<CoordinationEvent>(&data) {
                events.push((seq, event));
            }
        }

        Ok(events)
    }

    /// Replay all events from the beginning.
    pub fn replay_all(&self) -> Result<Vec<(u64, CoordinationEvent)>> {
        self.replay_from(0)
    }

    /// Compact the log by removing events before the given sequence number.
    /// Returns the number of events removed.
    pub fn compact(&self, before_sequence: u64) -> Result<usize> {
        let count = self.conn.execute(
            "DELETE FROM coordination_events WHERE sequence < ?1",
            params![before_sequence],
        )?;
        Ok(count)
    }

    /// Get the latest sequence number.
    pub fn latest_sequence(&self) -> Result<u64> {
        let seq: u64 = self.conn.query_row(
            "SELECT COALESCE(MAX(sequence), 0) FROM coordination_events",
            [],
            |row| row.get(0),
        )?;
        Ok(seq)
    }

    /// Get a snapshot of the event log state.
    pub fn snapshot(&self) -> Result<CoordinationSnapshot> {
        let latest_sequence = self.latest_sequence()?;
        let event_count: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM coordination_events",
            [],
            |row| row.get(0),
        )?;

        Ok(CoordinationSnapshot {
            latest_sequence,
            event_count,
            created_at: Utc::now(),
        })
    }

    /// Get the event type name for serialization.
    fn event_type_name(event: &CoordinationEvent) -> &'static str {
        match event {
            CoordinationEvent::SignalSent { .. } => "signal_sent",
            CoordinationEvent::SignalRead { .. } => "signal_read",
            CoordinationEvent::LeaseAcquired { .. } => "lease_acquired",
            CoordinationEvent::LeaseReleased { .. } => "lease_released",
            CoordinationEvent::LeaseRenewed { .. } => "lease_renewed",
            CoordinationEvent::LeaseExpired { .. } => "lease_expired",
            CoordinationEvent::ActionCreated { .. } => "action_created",
            CoordinationEvent::ActionStatusChanged { .. } => "action_status_changed",
            CoordinationEvent::ActionEdgeAdded { .. } => "action_edge_added",
            CoordinationEvent::TeamItemShared { .. } => "team_item_shared",
            CoordinationEvent::FileReserved { .. } => "file_reserved",
            CoordinationEvent::FileReleased { .. } => "file_released",
            CoordinationEvent::DeliveryQueued { .. } => "delivery_queued",
            CoordinationEvent::DeliveryInjected { .. } => "delivery_injected",
            CoordinationEvent::DeliveryAcknowledged { .. } => "delivery_acknowledged",
            CoordinationEvent::DeliveryFailed { .. } => "delivery_failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn open_log() -> (CoordinationEventLog, TempDir) {
        let dir = TempDir::new().unwrap();
        let log = CoordinationEventLog::open(&dir.path().join("events.db")).unwrap();
        (log, dir)
    }

    #[test]
    fn test_append_and_replay() {
        let (log, _dir) = open_log();

        let event = CoordinationEvent::SignalSent {
            signal_id: "sig-1".into(),
            from: "agent-a".into(),
            to: "agent-b".into(),
            signal_type: "info".into(),
        };

        let seq = log.append(&event).unwrap();
        assert_eq!(seq, 1);

        let events = log.replay_all().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, 1);
        assert_eq!(events[0].1, event);
    }

    #[test]
    fn test_append_batch() {
        let (log, _dir) = open_log();

        let events = vec![
            CoordinationEvent::ActionCreated {
                action_id: "act-1".into(),
                title: "Test".into(),
                status: "pending".into(),
                priority: 2,
            },
            CoordinationEvent::ActionStatusChanged {
                action_id: "act-1".into(),
                from: "pending".into(),
                to: "in_progress".into(),
            },
        ];

        let last_seq = log.append_batch(&events).unwrap();
        assert_eq!(last_seq, 2);

        let replayed = log.replay_all().unwrap();
        assert_eq!(replayed.len(), 2);
    }

    #[test]
    fn test_replay_from() {
        let (log, _dir) = open_log();

        for i in 0..5 {
            log.append(&CoordinationEvent::SignalSent {
                signal_id: format!("sig-{}", i),
                from: "a".into(),
                to: "b".into(),
                signal_type: "info".into(),
            })
            .unwrap();
        }

        let from_3 = log.replay_from(3).unwrap();
        assert_eq!(from_3.len(), 3);
        assert_eq!(from_3[0].0, 3);
    }

    #[test]
    fn test_compact() {
        let (log, _dir) = open_log();

        for i in 0..5 {
            log.append(&CoordinationEvent::SignalSent {
                signal_id: format!("sig-{}", i),
                from: "a".into(),
                to: "b".into(),
                signal_type: "info".into(),
            })
            .unwrap();
        }

        let removed = log.compact(3).unwrap();
        assert_eq!(removed, 2);

        let remaining = log.replay_all().unwrap();
        assert_eq!(remaining.len(), 3);
        assert_eq!(remaining[0].0, 3);
    }

    #[test]
    fn test_latest_sequence() {
        let (log, _dir) = open_log();
        assert_eq!(log.latest_sequence().unwrap(), 0);

        log.append(&CoordinationEvent::LeaseAcquired {
            lease_id: "l-1".into(),
            action_id: "a-1".into(),
            agent_id: "ag-1".into(),
            ttl_minutes: 10,
        })
        .unwrap();

        assert_eq!(log.latest_sequence().unwrap(), 1);
    }

    #[test]
    fn test_snapshot() {
        let (log, _dir) = open_log();

        log.append(&CoordinationEvent::ActionCreated {
            action_id: "a-1".into(),
            title: "Test".into(),
            status: "pending".into(),
            priority: 1,
        })
        .unwrap();

        let snap = log.snapshot().unwrap();
        assert_eq!(snap.latest_sequence, 1);
        assert_eq!(snap.event_count, 1);
    }

    #[test]
    fn test_all_event_types() {
        let (log, _dir) = open_log();

        let events = vec![
            CoordinationEvent::SignalSent {
                signal_id: "s1".into(),
                from: "a".into(),
                to: "b".into(),
                signal_type: "info".into(),
            },
            CoordinationEvent::SignalRead {
                signal_id: "s1".into(),
                agent_id: "b".into(),
            },
            CoordinationEvent::LeaseAcquired {
                lease_id: "l1".into(),
                action_id: "a1".into(),
                agent_id: "ag1".into(),
                ttl_minutes: 10,
            },
            CoordinationEvent::LeaseReleased {
                lease_id: "l1".into(),
                result: Some("done".into()),
            },
            CoordinationEvent::LeaseRenewed {
                lease_id: "l1".into(),
                extend_minutes: 5,
            },
            CoordinationEvent::LeaseExpired {
                lease_id: "l1".into(),
            },
            CoordinationEvent::ActionCreated {
                action_id: "a1".into(),
                title: "Test".into(),
                status: "pending".into(),
                priority: 2,
            },
            CoordinationEvent::ActionStatusChanged {
                action_id: "a1".into(),
                from: "pending".into(),
                to: "completed".into(),
            },
            CoordinationEvent::ActionEdgeAdded {
                from_id: "a1".into(),
                to_id: "a2".into(),
                edge_type: "depends_on".into(),
            },
            CoordinationEvent::TeamItemShared {
                item_id: "t1".into(),
                shared_by: "ag1".into(),
                item_type: "memory".into(),
            },
            CoordinationEvent::FileReserved {
                reservation_id: "r1".into(),
                path: "src/main.rs".into(),
                agent_id: "ag1".into(),
                mode: "exclusive".into(),
            },
            CoordinationEvent::FileReleased {
                reservation_id: "r1".into(),
                path: "src/main.rs".into(),
                agent_id: "ag1".into(),
            },
            CoordinationEvent::DeliveryQueued {
                signal_id: "s1".into(),
                to_agent: "b".into(),
            },
            CoordinationEvent::DeliveryInjected {
                signal_id: "s1".into(),
                turn_number: 1,
            },
            CoordinationEvent::DeliveryAcknowledged {
                signal_id: "s1".into(),
            },
            CoordinationEvent::DeliveryFailed {
                signal_id: "s1".into(),
                reason: "timeout".into(),
            },
        ];

        for event in &events {
            log.append(event).unwrap();
        }

        let replayed = log.replay_all().unwrap();
        assert_eq!(replayed.len(), 16);

        // Verify all event types round-trip correctly
        for (i, (_, event)) in replayed.iter().enumerate() {
            assert_eq!(*event, events[i]);
        }
    }
}
