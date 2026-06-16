//! Fallback chain for LLM providers with circuit breaker wrapping.
//!
//! 1:1 from mempalace's fallback-chain.ts:
//! - Try providers in configured order
//! - On error, try next provider
//! - Each wrapped in its own CircuitBreaker
//! - If all fail, throw last error

use super::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use super::provider::{LlmCompletion, LlmError, LlmProvider, ProviderFactory};
use std::sync::Arc;

/// A provider wrapped with its own circuit breaker.
struct WrappedProvider {
    provider: Arc<dyn LlmProvider>,
    circuit_breaker: Arc<CircuitBreaker>,
}

/// Fallback chain that tries providers in order, each with circuit breaker protection.
pub struct FallbackChain {
    providers: Vec<WrappedProvider>,
    /// `mr-ktc7`: optional re-resolve factories consulted when a provider
    /// returns `ModelNotFound`. Each factory builds a fresh provider from
    /// the current environment, so a freshly-exported `OPENAI_MODEL` or
    /// `ANTHROPIC_MODEL` will be picked up on the next attempt.
    re_resolve_factories: Vec<ProviderFactory>,
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
        Self {
            providers: wrapped,
            re_resolve_factories: Vec::new(),
        }
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
        Self {
            providers: wrapped,
            re_resolve_factories: Vec::new(),
        }
    }

    /// `mr-ktc7`: construct a chain that consults `factories` whenever a
    /// provider reports `LlmError::ModelNotFound`. Each factory is called
    /// in order; the first one that returns `Ok(provider)` retries the
    /// completion. If a factory itself returns `Err` (e.g. no API key set
    /// in this process), it is skipped silently so the chain can keep
    /// walking the list.
    pub fn with_re_resolve_factories(
        primary: Arc<dyn LlmProvider>,
        factories: Vec<ProviderFactory>,
    ) -> Self {
        let mut chain = Self::new(vec![primary]);
        chain.re_resolve_factories = factories;
        chain
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
                Err(LlmError::ModelNotFound { .. }) if !self.re_resolve_factories.is_empty() => {
                    // `mr-ktc7`: re-resolve to a fresh provider instead of
                    // counting this as a normal failure (the original
                    // provider was reachable, just wrong-model).
                    tracing::warn!(
                        target: "mempalace.llm",
                        provider = wp.provider.name(),
                        "model not found; consulting re-resolve factories"
                    );
                    for factory in &self.re_resolve_factories {
                        match factory() {
                            Ok(new_provider) => {
                                if !wp.circuit_breaker.allow_request().await {
                                    continue;
                                }
                                match new_provider.complete(system, user).await {
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
                            Err(e) => {
                                tracing::debug!(
                                    target: "mempalace.llm",
                                    error = %e,
                                    "re-resolve factory failed; trying next"
                                );
                            }
                        }
                    }
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
    use super::super::provider::LlmCompletion;
    use super::*;
    use async_trait::async_trait;

    /// Test provider that always returns `ModelNotFound` for `complete()`.
    /// Used to exercise the `mr-ktc7` re-resolve path.
    struct ModelNotFoundProvider;

    #[async_trait]
    impl LlmProvider for ModelNotFoundProvider {
        fn name(&self) -> &str {
            "model-not-found"
        }
        fn model(&self) -> &str {
            "missing-model"
        }
        async fn complete(&self, _system: &str, _user: &str) -> Result<LlmCompletion, LlmError> {
            Err(LlmError::ModelNotFound {
                provider: self.name().to_string(),
                model: self.model().to_string(),
            })
        }
        async fn check_available(&self) -> Result<(), String> {
            Ok(())
        }
    }

    /// `mr-ktc7`: when the primary reports `ModelNotFound` and we have
    /// re-resolve factories configured, the chain should walk the factory
    /// list, build a fresh provider from each, and return the first
    /// successful completion.
    #[tokio::test]
    async fn test_model_not_found_re_resolves_next_provider_from_env() {
        // Track how many times each factory was invoked
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_factory = Arc::clone(&calls);

        // First factory: a stub provider that returns a stub
        // completion from a fresh "env-resolved" model.
        struct FreshProvider;
        #[async_trait]
        impl LlmProvider for FreshProvider {
            fn name(&self) -> &str {
                "fresh-resolved"
            }
            fn model(&self) -> &str {
                "resolved-model"
            }
            async fn complete(
                &self,
                _system: &str,
                _user: &str,
            ) -> Result<LlmCompletion, LlmError> {
                Ok(LlmCompletion {
                    text: "fresh".to_string(),
                    model: self.model().to_string(),
                    provider: self.name().to_string(),
                    usage: None,
                })
            }
            async fn check_available(&self) -> Result<(), String> {
                Ok(())
            }
        }

        let factory: ProviderFactory = Box::new(move || {
            calls_for_factory.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(FreshProvider) as Arc<dyn LlmProvider>)
        });

        let primary: Arc<dyn LlmProvider> = Arc::new(ModelNotFoundProvider);
        let chain = FallbackChain::with_re_resolve_factories(primary, vec![factory]);

        let result = chain.complete("system", "user").await.unwrap();
        assert_eq!(result.text, "fresh");
        assert_eq!(result.provider, "fresh-resolved");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

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
