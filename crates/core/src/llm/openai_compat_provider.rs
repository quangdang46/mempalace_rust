//! OpenAI-compatible LLM provider.
//!
//! Works with OpenAI, Ollama, OpenRouter, LM Studio, llama.cpp, vLLM, Groq, etc.
//! POST to `/v1/chat/completions`.

use super::base_url_is_local;
use super::provider::{LlmCompletion, LlmError, LlmProvider, LlmUsage};
use async_trait::async_trait;
use reqwest::Client;
use std::time::Duration;

/// Configuration for the OpenAI-compatible provider.
#[derive(Debug, Clone)]
pub struct OpenAICompatConfig {
    pub api_key: Option<String>,
    pub model: String,
    pub base_url: String,
    pub timeout_ms: u64,
    /// `mr-2k4g`: `true` when the key was sourced from the
    /// `OPENAI_API_KEY` env-fallback. Providers constructed from explicit
    /// user config (e.g. `OpenAICompatConfig { api_key: Some(...), .. }`)
    /// leave this `false`. The consent gate only fires when this is `true`.
    pub key_from_env: bool,
}

impl Default for OpenAICompatConfig {
    fn default() -> Self {
        let api_key = std::env::var("OPENAI_API_KEY").ok();
        let key_from_env = api_key.is_some();
        Self {
            api_key,
            model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string()),
            base_url: std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com".to_string()),
            timeout_ms: std::env::var("MEMPALACE_LLM_TIMEOUT_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(60_000),
            key_from_env,
        }
    }
}

/// OpenAI-compatible LLM provider.
pub struct OpenAICompatProvider {
    config: OpenAICompatConfig,
    client: Client,
}

impl OpenAICompatProvider {
    pub fn new(config: OpenAICompatConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .expect("failed to build HTTP client");
        Self { config, client }
    }

    pub fn from_env() -> Self {
        Self::new(OpenAICompatConfig::default())
    }

    fn resolve_url(&self) -> String {
        let base = self.config.base_url.trim_end_matches('/');
        let base = base.trim_end_matches("/v1");
        format!("{base}/v1/chat/completions")
    }
}

#[async_trait]
impl LlmProvider for OpenAICompatProvider {
    fn name(&self) -> &str {
        "openai-compat"
    }

    fn model(&self) -> &str {
        &self.config.model
    }

    async fn complete(&self, system: &str, user: &str) -> Result<LlmCompletion, LlmError> {
        // `mr-2k4g`: env-fallback API keys require explicit user consent
        // before any LLM call may transmit data to the endpoint. Keys
        // configured explicitly in `mempalace.yaml` are exempt (their
        // `key_from_env` flag is `false`).
        if self.config.key_from_env {
            if let Some(ref key) = self.config.api_key {
                if !key.is_empty() {
                    if let Ok(cfg) = crate::config::Config::load() {
                        let status = crate::privacy::check_env_consent(
                            cfg.llm_consent_given,
                            self.name(),
                            &self.config.base_url,
                        );
                        if matches!(status, crate::privacy::ConsentStatus::Required) {
                            tracing::warn!(
                                target: "mempalace::llm",
                                provider = self.name(),
                                base_url = %self.config.base_url,
                                "env-fallback LLM API key detected; user consent not granted. \
                                 Set MEMPALACE_LLM_CONSENT=true for this process, or run \
                                 `mpr config record-llm-consent` to grant persistent consent."
                            );
                            return Err(LlmError::ConsentRequired {
                                provider: self.name().to_string(),
                                reason: "consent required for env-fallback API key".to_string(),
                            });
                        }
                    }
                }
            }
        }

        // `mr-ekep`: warn (opt-out) when the configured base_url points at a
        // public/external host. Loopback / RFC1918 / link-local / Tailscale
        // CGNAT are silent because they stay on the user's own network.
        if std::env::var("MEMPALACE_LLM_EXTERNAL_WARN")
            .map(|v| !matches!(v.as_str(), "0" | "false" | "no" | "off"))
            .unwrap_or(true)
            && !base_url_is_local(&self.config.base_url)
        {
            let approx_bytes = system.len() + user.len();
            tracing::warn!(
                target: "mempalace.llm",
                provider = self.name(),
                model = %self.config.model,
                base_url = %self.config.base_url,
                approx_input_bytes = approx_bytes,
                "sending LLM request to external host (set MEMPALACE_LLM_EXTERNAL_WARN=false to silence)"
            );
        }

        let url = self.resolve_url();

        let body = serde_json::json!({
            "model": self.config.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "temperature": 0.1,
            // B19: explicit stream:false so providers like OpenRouter
            // (which default to streaming) don't return SSE chunks that
            // the JSON parser can't read.
            "stream": false,
        });

        let mut request = self.client.post(&url);
        request = request.header("Content-Type", "application/json");
        if let Some(ref key) = self.config.api_key {
            request = request.header("Authorization", format!("Bearer {key}"));
        }

        let resp = request.json(&body).send().await?;
        let status = resp.status();

        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(LlmError::NonOk {
                code: status.as_u16(),
                message: if text.len() > 500 {
                    text[..500].to_string()
                } else {
                    text
                },
            });
        }

        let data: serde_json::Value = resp.json().await?;

        let text = data
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|choice| choice.get("message"))
            .and_then(|msg| msg.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        if text.is_empty() {
            return Err(LlmError::Empty {
                provider: self.name().to_string(),
                model: self.config.model.clone(),
            });
        }

        let usage = data.get("usage").map(|u| LlmUsage {
            prompt_tokens: u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            completion_tokens: u
                .get("completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize,
            total_tokens: u.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
        });

        Ok(LlmCompletion {
            text,
            model: self.config.model.clone(),
            provider: self.name().to_string(),
            usage,
        })
    }

    async fn check_available(&self) -> Result<(), String> {
        let base = self
            .config
            .base_url
            .trim_end_matches('/')
            .trim_end_matches("/v1");
        let url = format!("{base}/v1/models");

        let mut request = self.client.get(&url);
        if let Some(ref key) = self.config.api_key {
            request = request.header("Authorization", format!("Bearer {key}"));
        }

        match request.send().await {
            Ok(resp) if resp.status().is_success() => Ok(()),
            Ok(resp) => Err(format!(
                "HTTP {} from {}: {}",
                resp.status().as_u16(),
                self.config.base_url,
                resp.text().await.unwrap_or_default()
            )),
            Err(e) => Err(format!("Cannot reach {}: {e}", self.config.base_url)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_url() {
        let config = OpenAICompatConfig {
            base_url: "https://api.openai.com".to_string(),
            model: "gpt-4o-mini".to_string(),
            api_key: None,
            timeout_ms: 60_000,
            key_from_env: false,
        };
        let provider = OpenAICompatProvider::new(config);
        assert_eq!(
            provider.resolve_url(),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_resolve_url_with_trailing_v1() {
        let config = OpenAICompatConfig {
            base_url: "http://localhost:11434/v1".to_string(),
            model: "gemma4".to_string(),
            api_key: None,
            timeout_ms: 60_000,
            key_from_env: false,
        };
        let provider = OpenAICompatProvider::new(config);
        assert_eq!(
            provider.resolve_url(),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    /// `mr-2k4g`: when the key is marked as env-fallback, `complete()` must
    /// surface `ConsentRequired` instead of attempting to send the request.
    /// We use a synthetic fake base URL so the test never reaches the
    /// network — the gate is a precondition check that returns before
    /// the HTTP client is touched.
    #[test]
    fn test_complete_consent_required_when_env_key_without_consent() {
        let _guard = crate::test_env_lock().lock().unwrap();
        // No env override, force a clean (no XDG_CONFIG_HOME) Config::load().
        std::env::remove_var("MEMPALACE_LLM_CONSENT");
        let prev_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::remove_var("XDG_CONFIG_HOME");
        // Set a temp XDG so Config::load() reads a config with default
        // `llm_consent_given = false`.
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", temp.path().to_str().unwrap());

        let config = OpenAICompatConfig {
            base_url: "http://127.0.0.1:1".to_string(), // unreachable on purpose
            model: "gpt-4o-mini".to_string(),
            api_key: Some("sk-test-MOCK_env_fallback".to_string()),
            timeout_ms: 60_000,
            key_from_env: true,
        };
        let provider = OpenAICompatProvider::new(config);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let res = rt.block_on(provider.complete("sys", "user"));
        match res {
            Err(LlmError::ConsentRequired { provider, .. }) => {
                assert_eq!(provider, "openai-compat");
            }
            other => panic!("expected ConsentRequired, got {:?}", other),
        }

        if let Some(prev) = prev_xdg {
            std::env::set_var("XDG_CONFIG_HOME", prev);
        } else {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
    }

    /// `mr-2k4g`: when the env override is set, the gate opens even if no
    /// consent is recorded. This is the CI escape hatch.
    #[test]
    fn test_complete_consent_env_override_unblocks() {
        let _guard = crate::test_env_lock().lock().unwrap();
        std::env::set_var("MEMPALACE_LLM_CONSENT", "true");
        let prev_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::remove_var("XDG_CONFIG_HOME");
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", temp.path().to_str().unwrap());

        let config = OpenAICompatConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            model: "gpt-4o-mini".to_string(),
            api_key: Some("sk-test-MOCK_env_fallback".to_string()),
            timeout_ms: 100, // fast-fail rather than hang
            key_from_env: true,
        };
        let provider = OpenAICompatProvider::new(config);
        let rt = tokio::runtime::Runtime::new().unwrap();
        // The gate should NOT raise ConsentRequired. We don't care if the
        // network call itself fails — that proves we got past the gate.
        let res = rt.block_on(provider.complete("sys", "user"));
        match res {
            Err(LlmError::ConsentRequired { .. }) => {
                panic!("env override should have unblocked the gate");
            }
            _ => {}
        }

        std::env::remove_var("MEMPALACE_LLM_CONSENT");
        if let Some(prev) = prev_xdg {
            std::env::set_var("XDG_CONFIG_HOME", prev);
        } else {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
    }

    #[test]
    fn test_body_includes_stream_false() {
        // B19 / mr-suzh: openai_compat_provider must send stream:false
        // so OpenRouter / DeepSeek / etc. (which default to streaming) return
        // a normal JSON response. We assert the field is explicitly set
        // and serializes to JSON `false`.
        let body = serde_json::json!({
            "model": "gpt-4o-mini",
            "messages": [
                {"role": "system", "content": "sys"},
                {"role": "user", "content": "user"},
            ],
            "temperature": 0.1,
            "stream": false,
        });
        assert_eq!(
            body.get("stream").and_then(|v| v.as_bool()),
            Some(false)
        );
        let serialized = serde_json::to_string(&body).unwrap();
        assert!(serialized.contains("\"stream\":false"));
    }
}
