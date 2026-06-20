//! Live delivery system for inter-agent messages.
//!
//! Injects messages directly into idle agent sessions at turn boundaries,
//! bypassing the need for agents to poll their inbox.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::types::{Signal, SignalType};

/// Status of a pending delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveryStatus {
    /// Waiting to be injected.
    Queued,
    /// Injected into agent context.
    Injected,
    /// Agent acknowledged receipt.
    Acknowledged,
    /// Delivery failed.
    Failed(String),
}

/// A pending delivery waiting to be injected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingDelivery {
    pub signal_id: String,
    pub from_agent: String,
    pub to_agent: String,
    pub content: String,
    pub signal_type: String,
    pub created_at: DateTime<Utc>,
    pub status: DeliveryStatus,
}

/// Record of a completed delivery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryRecord {
    pub signal_id: String,
    pub from_agent: String,
    pub to_agent: String,
    pub injected_at: DateTime<Utc>,
    pub acknowledged_at: Option<DateTime<Utc>>,
    pub turn_number: Option<usize>,
}

/// Live delivery system for inter-agent messages.
pub struct LiveDelivery {
    /// Pending deliveries per agent.
    pending: Arc<RwLock<HashMap<String, Vec<PendingDelivery>>>>,
    /// Delivery history for ack tracking.
    history: Arc<RwLock<Vec<DeliveryRecord>>>,
}

impl LiveDelivery {
    /// Create a new live delivery system.
    pub fn new() -> Self {
        Self {
            pending: Arc::new(RwLock::new(HashMap::new())),
            history: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Queue a signal for live delivery to an agent.
    pub fn queue(&self, signal: &Signal) -> Result<()> {
        let delivery = PendingDelivery {
            signal_id: signal.id.clone(),
            from_agent: signal.from.clone(),
            to_agent: signal.to.clone(),
            content: signal.content.clone(),
            signal_type: format!("{:?}", signal.signal_type),
            created_at: Utc::now(),
            status: DeliveryStatus::Queued,
        };

        let mut pending = self
            .pending
            .write()
            .map_err(|e| anyhow::anyhow!("live_delivery pending lock poisoned: {}", e))?;
        pending
            .entry(signal.to.clone())
            .or_insert_with(Vec::new)
            .push(delivery);

        Ok(())
    }

    /// Poll for pending deliveries for an agent.
    /// Returns pending messages and marks them as Injected.
    pub fn poll(&self, agent_id: &str) -> Vec<PendingDelivery> {
        let mut pending = self
            .pending
            .write()
            .expect("live_delivery pending lock poisoned");
        if let Some(deliveries) = pending.get_mut(agent_id) {
            let injectable: Vec<PendingDelivery> = deliveries
                .iter()
                .filter(|d| d.status == DeliveryStatus::Queued)
                .cloned()
                .collect();

            // Mark as injected
            for delivery in deliveries.iter_mut() {
                if delivery.status == DeliveryStatus::Queued {
                    delivery.status = DeliveryStatus::Injected;
                }
            }

            // Record in history
            let mut history = self
                .history
                .write()
                .expect("live_delivery history lock poisoned");
            for delivery in &injectable {
                history.push(DeliveryRecord {
                    signal_id: delivery.signal_id.clone(),
                    from_agent: delivery.from_agent.clone(),
                    to_agent: delivery.to_agent.clone(),
                    injected_at: Utc::now(),
                    acknowledged_at: None,
                    turn_number: None,
                });
            }

            injectable
        } else {
            Vec::new()
        }
    }

    /// Acknowledge delivery of a signal.
    pub fn ack(&self, signal_id: &str) -> Result<()> {
        // Update pending status
        let mut pending = self
            .pending
            .write()
            .map_err(|e| anyhow::anyhow!("live_delivery pending lock poisoned: {}", e))?;
        for (_, deliveries) in pending.iter_mut() {
            for delivery in deliveries.iter_mut() {
                if delivery.signal_id == signal_id {
                    delivery.status = DeliveryStatus::Acknowledged;
                }
            }
        }

        // Update history
        let mut history = self
            .history
            .write()
            .map_err(|e| anyhow::anyhow!("live_delivery history lock poisoned: {}", e))?;
        for record in history.iter_mut() {
            if record.signal_id == signal_id && record.acknowledged_at.is_none() {
                record.acknowledged_at = Some(Utc::now());
            }
        }

        Ok(())
    }

    /// Requeue a failed delivery for retry.
    pub fn requeue(&self, signal_id: &str) -> Result<()> {
        let mut pending = self
            .pending
            .write()
            .map_err(|e| anyhow::anyhow!("live_delivery pending lock poisoned: {}", e))?;
        for (_, deliveries) in pending.iter_mut() {
            for delivery in deliveries.iter_mut() {
                if delivery.signal_id == signal_id
                    && (delivery.status == DeliveryStatus::Injected
                        || matches!(delivery.status, DeliveryStatus::Failed(_)))
                {
                    delivery.status = DeliveryStatus::Queued;
                }
            }
        }
        Ok(())
    }

    /// Mark a delivery as failed.
    pub fn mark_failed(&self, signal_id: &str, reason: &str) {
        let mut pending = self
            .pending
            .write()
            .expect("live_delivery pending lock poisoned");
        for (_, deliveries) in pending.iter_mut() {
            for delivery in deliveries.iter_mut() {
                if delivery.signal_id == signal_id {
                    delivery.status = DeliveryStatus::Failed(reason.to_string());
                }
            }
        }
    }

    /// Get count of pending deliveries for an agent.
    pub fn pending_count(&self, agent_id: &str) -> usize {
        let pending = self
            .pending
            .read()
            .expect("live_delivery pending read lock poisoned");
        pending
            .get(agent_id)
            .map(|d| {
                d.iter()
                    .filter(|d| d.status == DeliveryStatus::Queued)
                    .count()
            })
            .unwrap_or(0)
    }

    /// Get delivery history.
    pub fn history(&self) -> Vec<DeliveryRecord> {
        let history = self
            .history
            .read()
            .expect("live_delivery history read lock poisoned");
        history.clone()
    }

    /// Cleanup acknowledged deliveries from pending.
    pub fn cleanup(&self) {
        let mut pending = self
            .pending
            .write()
            .expect("live_delivery pending lock poisoned");
        for (_, deliveries) in pending.iter_mut() {
            deliveries.retain(|d| d.status != DeliveryStatus::Acknowledged);
        }
    }

    /// Format pending deliveries as XML peer_message envelopes.
    pub fn format_envelope(deliveries: &[PendingDelivery]) -> String {
        let mut envelopes = Vec::new();

        for delivery in deliveries {
            let signal_type = &delivery.signal_type;
            let escaped_content = xml_escape(&delivery.content);
            let escaped_from = xml_escape(&delivery.from_agent);

            envelopes.push(format!(
                "<peer_message from=\"{}\" signal_id=\"{}\" type=\"{}\" timestamp=\"{}\">\n{}\n</peer_message>",
                escaped_from,
                delivery.signal_id,
                signal_type,
                delivery.created_at.timestamp_millis(),
                escaped_content
            ));
        }

        envelopes.join("\n\n")
    }
}

impl Default for LiveDelivery {
    fn default() -> Self {
        Self::new()
    }
}

/// Escape XML special characters.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_signal(id: &str, from: &str, to: &str, content: &str) -> Signal {
        Signal {
            id: id.to_string(),
            from: from.to_string(),
            to: to.to_string(),
            thread_id: None,
            reply_to: None,
            signal_type: SignalType::Info,
            content: content.to_string(),
            metadata: HashMap::new(),
            created_at: Utc::now(),
            read_at: None,
            expires_at: None,
        }
    }

    #[test]
    fn test_queue_and_poll() {
        let delivery = LiveDelivery::new();
        let signal = make_signal("s1", "agent-a", "agent-b", "Hello from A");

        delivery.queue(&signal).unwrap();
        assert_eq!(delivery.pending_count("agent-b"), 1);

        let pending = delivery.poll("agent-b");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].signal_id, "s1");
        assert_eq!(pending[0].content, "Hello from A");

        // Should be empty after poll
        assert_eq!(delivery.pending_count("agent-b"), 0);
    }

    #[test]
    fn test_ack() {
        let delivery = LiveDelivery::new();
        let signal = make_signal("s1", "agent-a", "agent-b", "Hello");

        delivery.queue(&signal).unwrap();
        delivery.poll("agent-b");
        delivery.ack("s1").unwrap();

        let history = delivery.history();
        assert_eq!(history.len(), 1);
        assert!(history[0].acknowledged_at.is_some());
    }

    #[test]
    fn test_requeue() {
        let delivery = LiveDelivery::new();
        let signal = make_signal("s1", "agent-a", "agent-b", "Hello");

        delivery.queue(&signal).unwrap();
        delivery.poll("agent-b");
        delivery.mark_failed("s1", "timeout");
        delivery.requeue("s1").unwrap();

        // Should be available for poll again
        let pending = delivery.poll("agent-b");
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn test_format_envelope() {
        let deliveries = vec![PendingDelivery {
            signal_id: "s1".to_string(),
            from_agent: "agent-a".to_string(),
            to_agent: "agent-b".to_string(),
            content: "Hello <world>".to_string(),
            signal_type: "Info".to_string(),
            created_at: Utc::now(),
            status: DeliveryStatus::Queued,
        }];

        let envelope = LiveDelivery::format_envelope(&deliveries);
        assert!(envelope.contains("<peer_message"));
        assert!(envelope.contains("agent-a"));
        assert!(envelope.contains("Hello &lt;world&gt;"));
        assert!(envelope.contains("</peer_message>"));
    }

    #[test]
    fn test_cleanup() {
        let delivery = LiveDelivery::new();
        let signal = make_signal("s1", "agent-a", "agent-b", "Hello");

        delivery.queue(&signal).unwrap();
        delivery.poll("agent-b");
        delivery.ack("s1").unwrap();
        delivery.cleanup();

        // Pending should be empty after cleanup
        let pending = delivery
            .pending
            .read()
            .expect("test pending read lock poisoned");
        assert!(pending.get("agent-b").map_or(true, |d| d.is_empty()));
    }
}
