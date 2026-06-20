//! Anthropic Claude provider.
//!
//! POST to Anthropic Messages API + embeddings endpoint.

use super::base_url_is_local;
use super::provider::{LlmCompletion, LlmError, LlmProvider, LlmUsage};
use async_trait::async_trait;
use reqwest::Client;
use std::time::Duration;

const ANTHROPIC_API_VERSION: &str = "2023-06-01";

/// Configuration for the Anthropic provider.
#[derive(Debug, Clone)]
pub struct AnthropicConfig {
    pub api_key: Option<String>,
    pub model: String,
    pub timeout_ms: u64,
    /// `mr-2k4g`: `true` when the key was sourced from the
    /// `ANTHROPIC_API_KEY` env-fallback. Providers constructed from
    /// explicit user config (e.g. `AnthropicConfig { api_key: Some(...), .. }`)
    /// leave this `false`. The consent gate only fires when this is `true`.
    pub key_from_env: bool,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        let api_key = std::env::var("ANTHROPIC_API_KEY").ok();
        let key_from_env = api_key.is_some();
        Self {
            api_key,
            model: std::env::var("ANTHROPIC_MODEL")
                .unwrap_or_else(|_| "claude-sonnet-4-20250514".to_string()),
            timeout_ms: std::env::var("MEMPALACE_LLM_TIMEOUT_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(60_000),
            key_from_env,
        }
    }
}

/// Anthropic Claude LLM provider.
pub struct AnthropicProvider {
    config: AnthropicConfig,
    client: Client,
}

impl AnthropicProvider {
    pub fn new(config: AnthropicConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .expect("failed to build HTTP client");
        Self { config, client }
    }

    pub fn from_env() -> Self {
        Self::new(AnthropicConfig::default())
    }

    /// Get the embedding model name from config or environment.
    fn embedding_model(&self) -> String {
        std::env::var("ANTHROPIC_EMBEDDING_MODEL").unwrap_or_else(|_| "voyage-3.5".to_string())
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn model(&self) -> &str {
        &self.config.model
    }

    async fn complete(&self, system: &str, user: &str) -> Result<LlmCompletion, LlmError> {
        // `mr-2k4g`: env-fallback API keys require explicit user consent
        // before any LLM call may transmit data to the endpoint. Keys
        // configured explicitly in `mempalace.yaml` are exempt.
        if self.config.key_from_env {
            if let Some(ref key) = self.config.api_key {
                if !key.is_empty() {
                    if let Ok(cfg) = crate::config::Config::load() {
                        let status = crate::privacy::check_env_consent(
                            cfg.llm_consent_given,
                            self.name(),
                            "https://api.anthropic.com",
                        );
                        if matches!(status, crate::privacy::ConsentStatus::Required) {
                            tracing::warn!(
                                target: "mempalace::llm",
                                provider = self.name(),
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

        // `mr-ekep`: warn (opt-out) when the call leaves the user's network.
        // Anthropic's API URL is hardcoded, so the check is deterministic:
        // api.anthropic.com is always external.
        if std::env::var("MEMPALACE_LLM_EXTERNAL_WARN")
            .map(|v| !matches!(v.as_str(), "0" | "false" | "no" | "off"))
            .unwrap_or(true)
            && !base_url_is_local("https://api.anthropic.com")
        {
            let approx_bytes = system.len() + user.len();
            tracing::warn!(
                target: "mempalace.llm",
                provider = self.name(),
                model = %self.config.model,
                base_url = "https://api.anthropic.com",
                approx_input_bytes = approx_bytes,
                "sending LLM request to external host (set MEMPALACE_LLM_EXTERNAL_WARN=false to silence)"
            );
        }

        let api_key = self
            .config
            .api_key
            .as_ref()
            .ok_or_else(|| LlmError::MissingApiKey {
                provider: self.name().to_string(),
            })?;

        let body = serde_json::json!({
            "model": self.config.model,
            "max_tokens": 4096,
            "temperature": 0.1,
            "system": system,
            "messages": [{"role": "user", "content": user}],
        });

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("Content-Type", "application/json")
            .header("X-API-Key", api_key)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .json(&body)
            .send()
            .await?;

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

        let text: String = data
            .get("content")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect()
            })
            .unwrap_or_default();

        if text.is_empty() {
            return Err(LlmError::Empty {
                provider: self.name().to_string(),
                model: self.config.model.clone(),
            });
        }

        let usage = data.get("usage").map(|u| LlmUsage {
            prompt_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            completion_tokens: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0)
                as usize,
            total_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as usize
                + u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
        });

        Ok(LlmCompletion {
            text,
            model: self.config.model.clone(),
            provider: self.name().to_string(),
            usage,
        })
    }

    async fn describe_image(
        &self,
        image_base64: &str,
        mime: &str,
        prompt: &str,
    ) -> Result<LlmCompletion, LlmError> {
        let api_key = self
            .config
            .api_key
            .as_ref()
            .ok_or_else(|| LlmError::MissingApiKey {
                provider: self.name().to_string(),
            })?;

        let body = serde_json::json!({
            "model": self.config.model,
            "max_tokens": 1024,
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "image", "source": {
                        "type": "base64",
                        "media_type": mime,
                        "data": image_base64,
                    }},
                    {"type": "text", "text": prompt},
                ],
            }],
        });

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("Content-Type", "application/json")
            .header("X-API-Key", api_key)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .json(&body)
            .send()
            .await?;

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

        let text: String = data
            .get("content")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect()
            })
            .unwrap_or_default();

        if text.is_empty() {
            return Err(LlmError::Empty {
                provider: self.name().to_string(),
                model: self.config.model.clone(),
            });
        }

        Ok(LlmCompletion {
            text,
            model: self.config.model.clone(),
            provider: self.name().to_string(),
            usage: None,
        })
    }

    async fn check_available(&self) -> Result<(), String> {
        if self.config.api_key.is_none() {
            return Err("ANTHROPIC_API_KEY not set".to_string());
        }
        Ok(())
    }

    async fn embed_text(&self, text: &str) -> Result<Vec<f32>, LlmError> {
        let api_key = self
            .config
            .api_key
            .as_ref()
            .ok_or_else(|| LlmError::MissingApiKey {
                provider: self.name().to_string(),
            })?;

        let embedding_model = self.embedding_model();
        let body = serde_json::json!({
            "model": embedding_model,
            "input": text,
        });

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/embeddings")
            .header("Content-Type", "application/json")
            .header("X-API-Key", api_key)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text_err = resp.text().await.unwrap_or_default();
            return Err(LlmError::NonOk {
                code: status.as_u16(),
                message: if text_err.len() > 500 {
                    text_err[..500].to_string()
                } else {
                    text_err
                },
            });
        }

        let data: serde_json::Value = resp.json().await?;

        let embedding = data
            .get("embedding")
            .and_then(|e| e.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_f64())
                    .map(|f| f as f32)
                    .collect()
            })
            .ok_or_else(|| LlmError::Shape("no embedding array in response".to_string()))?;

        Ok(embedding)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_key_returns_error() {
        let config = AnthropicConfig {
            api_key: None,
            model: "claude-sonnet-4-20250514".to_string(),
            timeout_ms: 60_000,
            key_from_env: false,
        };
        let provider = AnthropicProvider::new(config);
        // check_available should fail without key
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert!(rt.block_on(provider.check_available()).is_err());
    }

    /// `mr-2k4g`: when the Anthropic key is marked as env-fallback, the
    /// consent gate must raise `ConsentRequired` instead of sending data to
    /// api.anthropic.com. The check runs before the HTTP client is touched.
    #[test]
    fn test_complete_consent_required_when_env_key_without_consent() {
        let _guard = crate::test_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("MEMPALACE_LLM_CONSENT");
        let prev_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::remove_var("XDG_CONFIG_HOME");
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", temp.path().to_str().unwrap());

        let config = AnthropicConfig {
            api_key: Some("sk-ant-test-MOCK_env_fallback".to_string()),
            model: "claude-sonnet-4-20250514".to_string(),
            timeout_ms: 60_000,
            key_from_env: true,
        };
        let provider = AnthropicProvider::new(config);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let res = rt.block_on(provider.complete("sys", "user"));
        match res {
            Err(LlmError::ConsentRequired { provider, .. }) => {
                assert_eq!(provider, "anthropic");
            }
            other => panic!("expected ConsentRequired, got {:?}", other),
        }

        if let Some(prev) = prev_xdg {
            std::env::set_var("XDG_CONFIG_HOME", prev);
        } else {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
    }

    /// `mr-2k4g`: env override `MEMPALACE_LLM_CONSENT=true` unblocks the
    /// gate even when no persisted consent exists.
    #[test]
    fn test_complete_consent_env_override_unblocks() {
        let _guard = crate::test_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("MEMPALACE_LLM_CONSENT", "true");
        let prev_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::remove_var("XDG_CONFIG_HOME");
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", temp.path().to_str().unwrap());

        let config = AnthropicConfig {
            api_key: Some("sk-ant-test-MOCK_env_fallback".to_string()),
            model: "claude-sonnet-4-20250514".to_string(),
            timeout_ms: 100, // fast-fail
            key_from_env: true,
        };
        let provider = AnthropicProvider::new(config);
        let rt = tokio::runtime::Runtime::new().unwrap();
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
}
