//! OpenAI-compatible LLM provider.
//!
//! Works with OpenAI, Ollama, OpenRouter, LM Studio, llama.cpp, vLLM, Groq, etc.
//! POST to `/v1/chat/completions`.

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
}

impl Default for OpenAICompatConfig {
    fn default() -> Self {
        Self {
            api_key: std::env::var("OPENAI_API_KEY").ok(),
            model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string()),
            base_url: std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com".to_string()),
            timeout_ms: std::env::var("MEMPALACE_LLM_TIMEOUT_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(60_000),
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
        let url = self.resolve_url();

        let body = serde_json::json!({
            "model": self.config.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "temperature": 0.1,
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
        };
        let provider = OpenAICompatProvider::new(config);
        assert_eq!(
            provider.resolve_url(),
            "http://localhost:11434/v1/chat/completions"
        );
    }
}
