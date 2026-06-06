//! Fallback chain for LLM providers with circuit breaker wrapping.
//!
//! 1:1 from mempalace's fallback-chain.ts:
//! - Try providers in configured order
//! - On error, try next provider
//! - Each wrapped in its own CircuitBreaker
//! - If all fail, throw last error

use super::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use super::provider::{LlmCompletion, LlmError, LlmProvider};
use std::sync::Arc;

/// A provider wrapped with its own circuit breaker.
struct WrappedProvider {
    provider: Arc<dyn LlmProvider>,
    circuit_breaker: Arc<CircuitBreaker>,
}

/// Fallback chain that tries providers in order, each with circuit breaker protection.
pub struct FallbackChain {
    providers: Vec<WrappedProvider>,
}

impl FallbackChain {
    /// Create a new fallback chain from a list of providers.
    /// Each provider gets its own circuit breaker with default config.
    pub fn new(providers: Vec<Arc<dyn LlmProvider>>) -> Self {
        let wrapped = providers
            .into_iter()
            .map(|p| {
                let name = p.name().to_string();
                WrappedProvider {
                    circuit_breaker: Arc::new(CircuitBreaker::new(
                        name,
                        CircuitBreakerConfig::default(),
                    )),
                    provider: p,
                }
            })
            .collect();
        Self { providers: wrapped }
    }

    /// Create with custom circuit breaker config.
    pub fn with_config(providers: Vec<Arc<dyn LlmProvider>>, config: CircuitBreakerConfig) -> Self {
        let wrapped = providers
            .into_iter()
            .map(|p| {
                let name = p.name().to_string();
                WrappedProvider {
                    circuit_breaker: Arc::new(CircuitBreaker::new(name, config.clone())),
                    provider: p,
                }
            })
            .collect();
        Self { providers: wrapped }
    }

    /// Execute a completion through the fallback chain.
    /// Returns the first successful result, or the last error if all fail.
    pub async fn complete(&self, system: &str, user: &str) -> Result<LlmCompletion, LlmError> {
        let mut last_error = None;

        for wp in &self.providers {
            if !wp.circuit_breaker.allow_request().await {
                continue; // circuit open, skip this provider
            }

            match wp.provider.complete(system, user).await {
                Ok(result) => {
                    wp.circuit_breaker.record_success();
                    return Ok(result);
                }
                Err(e) => {
                    wp.circuit_breaker.record_failure().await;
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| LlmError::Empty {
            provider: "fallback_chain".to_string(),
            model: "none".to_string(),
        }))
    }

    /// Get the number of configured providers.
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::super::noop_provider::NoopProvider;
    use super::*;

    #[tokio::test]
    async fn test_single_provider() {
        let noop = Arc::new(NoopProvider::default());
        let chain = FallbackChain::new(vec![noop]);
        let result = chain.complete("system", "user").await.unwrap();
        assert_eq!(result.text, "");
        assert_eq!(result.provider, "noop");
    }

    #[tokio::test]
    async fn test_empty_chain() {
        let chain = FallbackChain::new(vec![]);
        let result = chain.complete("system", "user").await;
        assert!(result.is_err());
    }
}
