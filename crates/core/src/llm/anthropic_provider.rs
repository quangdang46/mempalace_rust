//! Anthropic Claude provider.
//!
//! POST to Anthropic Messages API + embeddings endpoint.

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
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
            model: std::env::var("ANTHROPIC_MODEL")
                .unwrap_or_else(|_| "claude-sonnet-4-20250514".to_string()),
            timeout_ms: std::env::var("MEMPALACE_LLM_TIMEOUT_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(60_000),
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
        };
        let provider = AnthropicProvider::new(config);
        // check_available should fail without key
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert!(rt.block_on(provider.check_available()).is_err());
    }
}
