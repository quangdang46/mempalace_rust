//! LLM provider abstraction layer.
//!
//! Provides:
//! - `LlmProvider` trait for async text/image completion
//! - `CircuitBreaker` for provider resilience (3-failure threshold)
//! - `FallbackChain` for automatic provider failover
//! - Concrete providers: OpenAI-compatible, Anthropic, Noop

pub mod anthropic_provider;
pub mod circuit_breaker;
pub mod fallback_chain;
pub mod minimax_provider;
pub mod noop_provider;
pub mod openai_compat_provider;
pub mod provider;

pub use anthropic_provider::{AnthropicConfig, AnthropicProvider};
pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
pub use fallback_chain::FallbackChain;
pub use minimax_provider::{MinimaxConfig, MinimaxProvider};
pub use noop_provider::NoopProvider;
pub use openai_compat_provider::{OpenAICompatConfig, OpenAICompatProvider};
pub use provider::{LlmCompletion, LlmError, LlmProvider, LlmUsage};

use std::sync::Arc;

/// Returns true if `url` points to a local/private network (loopback, RFC1918,
/// link-local, Tailscale CGNAT 100.64/10). External public URLs return false.
///
/// `mr-ekep`: this powers the external-LLM privacy warning. The check is
/// host-only (port and path are ignored) so callers can pass a full chat
/// completions URL or a bare base URL.
pub fn base_url_is_local(url: &str) -> bool {
    // Strip scheme
    let after_scheme = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url);
    // Drop path/query/fragment
    let host_port = after_scheme.split('/').next().unwrap_or(after_scheme);
    let host_port = host_port.split('?').next().unwrap_or(host_port);
    let host_port = host_port.split('#').next().unwrap_or(host_port);
    // Strip :port
    let host = if let Some(stripped) = host_port.strip_prefix('[') {
        // IPv6 literal: [::1]:8080
        if let Some(end) = stripped.find(']') {
            &stripped[..end]
        } else {
            stripped
        }
    } else if let Some(colon) = host_port.rfind(':') {
        &host_port[..colon]
    } else {
        host_port
    };

    let lower = host.to_lowercase();
    if lower == "localhost" {
        return true;
    }
    if lower == "::1" {
        return true;
    }
    if let Ok(ip) = lower.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => {
                v4.is_loopback()                  // 127.0.0.0/8
                    || v4.is_private()            // 10/8, 172.16/12, 192.168/16
                    || v4.is_link_local()         // 169.254/16
                    || is_tailscale_cgnat_v4(v4)  // 100.64/10
            }
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.segments()[0] == 0xfc00 || v6.segments()[0] == 0xfd00 // ULA fc00::/7
                    || (v6.segments()[0] == 0xfe80) // link-local fe80::/10 (covers fe80..febf)
            }
        };
    }
    false
}

/// `mr-ekep`: Tailscale CGNAT range is 100.64.0.0/10.
fn is_tailscale_cgnat_v4(ip: std::net::Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 100 && (octets[1] >= 64 && octets[1] <= 127)
}

/// Create an LLM provider from environment variables.
///
/// Priority order:
/// 1. Anthropic (if ANTHROPIC_API_KEY is set)
/// 2. OpenAI-compatible (if OPENAI_API_KEY or OPENAI_BASE_URL is set)
/// 3. Noop (fallback — returns empty completions)
///
/// Returns `Arc<dyn LlmProvider>` for use behind `Arc`.
pub fn create_llm_provider_from_env() -> Arc<dyn LlmProvider> {
    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        Arc::new(AnthropicProvider::from_env())
    } else if std::env::var("OPENAI_API_KEY").is_ok() || std::env::var("OPENAI_BASE_URL").is_ok() {
        Arc::new(OpenAICompatProvider::from_env())
    } else {
        Arc::new(NoopProvider::new())
    }
}

/// Create a fallback chain from environment-configured providers.
///
/// Tries Anthropic first, then OpenAI-compatible, then Noop.
pub fn create_fallback_chain_from_env() -> FallbackChain {
    let mut providers: Vec<Arc<dyn LlmProvider>> = Vec::new();

    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        providers.push(Arc::new(AnthropicProvider::from_env()));
    }

    if std::env::var("OPENAI_API_KEY").is_ok() || std::env::var("OPENAI_BASE_URL").is_ok() {
        providers.push(Arc::new(OpenAICompatProvider::from_env()));
    }

    // Always include noop as last resort
    providers.push(Arc::new(NoopProvider::new()));

    FallbackChain::new(providers)
}

#[cfg(test)]
mod base_url_is_local_tests {
    use super::base_url_is_local;

    #[test]
    fn test_base_url_is_local_loopback() {
        assert!(base_url_is_local("http://127.0.0.1:8080"));
        assert!(base_url_is_local("http://localhost:11434"));
        assert!(base_url_is_local("http://[::1]:8080"));
    }

    #[test]
    fn test_base_url_is_local_rfc1918() {
        assert!(base_url_is_local("http://10.0.0.5"));
        assert!(base_url_is_local("http://192.168.1.1"));
        assert!(base_url_is_local("http://172.16.0.1"));
        assert!(base_url_is_local("http://172.31.255.254"));
    }

    #[test]
    fn test_base_url_is_local_tailscale_cgnat() {
        assert!(base_url_is_local("http://100.64.0.1"));
        assert!(base_url_is_local("http://100.127.255.254"));
    }

    #[test]
    fn test_base_url_is_external_for_public_apis() {
        assert!(!base_url_is_local("https://api.openai.com"));
        assert!(!base_url_is_local("https://api.anthropic.com"));
    }
}
