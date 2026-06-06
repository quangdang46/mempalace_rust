//! Circuit breaker state machine for LLM provider resilience.
//!
//! 1:1 from mempalace's circuit-breaker.ts:
//! - States: Closed, Open, HalfOpen
//! - Constants: failure_threshold=3, failure_window_ms=60_000, recovery_timeout_ms=30_000
//! - Closed: allow all. On >=3 failures in 60s window -> Open
//! - Open: deny all. After 30s -> HalfOpen
//! - HalfOpen: allow single probe. Success -> Closed, Failure -> Open

use crate::types::{CircuitBreakerState as CircuitBreakerData, CircuitState};
use std::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};
use tokio::sync::Mutex;

/// Configuration for the circuit breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    pub failure_threshold: usize,
    pub failure_window_ms: u64,
    pub recovery_timeout_ms: u64,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            failure_window_ms: 60_000,
            recovery_timeout_ms: 30_000,
        }
    }
}

/// Thread-safe circuit breaker for a single provider.
pub struct CircuitBreaker {
    provider_name: String,
    config: CircuitBreakerConfig,
    state: AtomicU8, // 0=Closed, 1=Open, 2=HalfOpen
    failure_count: AtomicUsize,
    last_failure_at: AtomicU64, // epoch millis
    last_success_at: AtomicU64,
    failures_in_window: Mutex<Vec<u64>>, // timestamps of recent failures
}

impl CircuitBreaker {
    pub fn new(provider_name: impl Into<String>, config: CircuitBreakerConfig) -> Self {
        Self {
            provider_name: provider_name.into(),
            config,
            state: AtomicU8::new(0), // Closed
            failure_count: AtomicUsize::new(0),
            last_failure_at: AtomicU64::new(0),
            last_success_at: AtomicU64::new(0),
            failures_in_window: Mutex::new(Vec::new()),
        }
    }

    /// Check if a request is allowed through the circuit breaker.
    pub async fn allow_request(&self) -> bool {
        let state = self.state.load(Ordering::SeqCst);

        match state {
            0 => true, // Closed
            1 => {
                let now = now_millis();
                let last_failure = self.last_failure_at.load(Ordering::SeqCst);
                if last_failure > 0 && (now - last_failure) >= self.config.recovery_timeout_ms {
                    self.state.store(2, Ordering::SeqCst);
                    true
                } else {
                    false
                }
            }
            2 => true, // HalfOpen
            _ => unreachable!(),
        }
    }

    /// Record a successful request. Transitions HalfOpen -> Closed.
    pub fn record_success(&self) {
        let now = now_millis();
        self.last_success_at.store(now, Ordering::SeqCst);
        let prev_state = self.state.swap(0, Ordering::SeqCst);
        if prev_state == 2 {
            self.failure_count.store(0, Ordering::SeqCst);
        }
    }

    /// Record a failed request. May transition Closed -> Open.
    pub async fn record_failure(&self) {
        let now = now_millis();
        self.last_failure_at.store(now, Ordering::SeqCst);
        self.failure_count.fetch_add(1, Ordering::SeqCst);

        {
            let mut failures = self.failures_in_window.lock().await;
            failures.push(now);
            let cutoff = now.saturating_sub(self.config.failure_window_ms);
            failures.retain(|&ts| ts > cutoff);

            if failures.len() >= self.config.failure_threshold {
                self.state.store(1, Ordering::SeqCst);
            }
        }
    }

    /// Get the current state as data struct.
    pub async fn get_state(&self) -> CircuitBreakerData {
        let state = match self.state.load(Ordering::SeqCst) {
            0 => CircuitState::Closed,
            1 => CircuitState::Open,
            _ => CircuitState::HalfOpen,
        };

        CircuitBreakerData {
            state,
            failure_count: self.failure_count.load(Ordering::SeqCst),
            last_failure_at: {
                let ts = self.last_failure_at.load(Ordering::SeqCst);
                if ts > 0 {
                    Some(chrono::DateTime::from_timestamp_millis(ts as i64).unwrap_or_default())
                } else {
                    None
                }
            },
            last_success_at: {
                let ts = self.last_success_at.load(Ordering::SeqCst);
                if ts > 0 {
                    Some(chrono::DateTime::from_timestamp_millis(ts as i64).unwrap_or_default())
                } else {
                    None
                }
            },
            failure_window_ms: self.config.failure_window_ms,
            recovery_timeout_ms: self.config.recovery_timeout_ms,
            failure_threshold: self.config.failure_threshold,
        }
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_closed_allows_requests() {
        let cb = CircuitBreaker::new("test", CircuitBreakerConfig::default());
        assert!(cb.allow_request().await);
    }

    #[tokio::test]
    async fn test_opens_after_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            failure_window_ms: 60_000,
            recovery_timeout_ms: 100,
        };
        let cb = CircuitBreaker::new("test", config);
        cb.record_failure().await;
        cb.record_failure().await;
        assert!(!cb.allow_request().await);
    }

    #[tokio::test]
    async fn test_halfopen_after_recovery_timeout() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            failure_window_ms: 60_000,
            recovery_timeout_ms: 50,
        };
        let cb = CircuitBreaker::new("test", config);
        cb.record_failure().await;
        assert!(!cb.allow_request().await);
        tokio::time::sleep(tokio::time::Duration::from_millis(60)).await;
        assert!(cb.allow_request().await);
    }

    #[tokio::test]
    async fn test_success_resets_from_halfopen() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            failure_window_ms: 60_000,
            recovery_timeout_ms: 50,
        };
        let cb = CircuitBreaker::new("test", config);
        cb.record_failure().await;
        tokio::time::sleep(tokio::time::Duration::from_millis(60)).await;
        cb.allow_request().await;
        cb.record_success();
        assert!(cb.allow_request().await);
        let state = cb.get_state().await;
        assert!(matches!(state.state, CircuitState::Closed));
    }
}
