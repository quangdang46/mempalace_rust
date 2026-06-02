//! Minimax (minimax.io) LLM provider.
//!
//! Anthropic-compatible API at `${MINIMAX_BASE_URL}/v1/messages`.
//! Uses raw reqwest (no SDK) to avoid x-stainless-* headers the gateway rejects.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;

use super::provider::{LlmCompletion, LlmError, LlmProvider, LlmUsage};

const MINIMAX_API_VERSION: &str = "2023-06-01";
const DEFAULT_BASE_URL: &str = "https://api.minimax.io/anthropic";
const DEFAULT_MODEL: &str = "MiniMax-M2.7";
const DEFAULT_MAX_TOKENS: u32 = 800;
const HARD_CAP_MAX_TOKENS: u32 = 800;

/// Configuration for the Minimax provider.
#[derive(Debug, Clone)]
pub struct MinimaxConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub max_tokens: u32,
    pub timeout: Duration,
}

impl Default for MinimaxConfig {
    fn default() -> Self {
        Self {
            api_key: std::env::var("MINIMAX_API_KEY").unwrap_or_default(),
            base_url: std::env::var("MINIMAX_BASE_URL")
                .unwrap_or_else(|_| DEFAULT_BASE_URL.to_string()),
            model: std::env::var("MINIMAX_MODEL")
                .unwrap_or_else(|_| DEFAULT_MODEL.to_string()),
            max_tokens: std::env::var("MINIMAX_MAX_TOKENS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_MAX_TOKENS),
            timeout: Duration::from_secs(60),
        }
    }
}

/// Minimax LLM provider.
pub struct MinimaxProvider {
    config: MinimaxConfig,
    client: Client,
}

impl MinimaxProvider {
    pub fn new(config: MinimaxConfig) -> Result<Self, LlmError> {
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(LlmError::Http)?;
        Ok(Self { config, client })
    }

    pub fn from_env() -> Result<Self, LlmError> {
        let config = MinimaxConfig::default();
        if config.api_key.is_empty() {
            return Err(LlmError::MissingApiKey {
                provider: "minimax".to_string(),
            });
        }
        Self::new(config)
    }

    fn base_url(&self) -> String {
        format!("{}/v1/messages", self.config.base_url.trim_end_matches('/'))
    }

    fn max_tokens_for_request(&self) -> u32 {
        self.config.max_tokens.min(HARD_CAP_MAX_TOKENS)
    }
}

#[async_trait]
impl LlmProvider for MinimaxProvider {
    async fn complete(&self, system: &str, user: &str) -> Result<LlmCompletion, LlmError> {
        #[cfg(feature = "telemetry")]
        let _telemetry_start = std::time::Instant::now();

        let body = MinimaxRequest {
            model: &self.config.model,
            max_tokens: self.max_tokens_for_request(),
            system,
            messages: &[MinimaxMessage {
                role: "user",
                content: user,
            }],
        };

        let resp = self
            .client
            .post(self.base_url())
            .header("Content-Type", "application/json")
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", MINIMAX_API_VERSION)
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

        let data: MinimaxResponse = resp.json().await?;

        let text: String = data
            .content
            .iter()
            .filter(|c| c.kind == "text")
            .map(|c| c.text.as_str())
            .collect();

        if text.is_empty() {
            return Err(LlmError::Empty {
                provider: self.name().to_string(),
                model: self.config.model.clone(),
            });
        }

        let usage = data.usage.map(|u| LlmUsage {
            prompt_tokens: u.input_tokens as usize,
            completion_tokens: u.output_tokens as usize,
            total_tokens: u.input_tokens as usize + u.output_tokens as usize,
        });

        Ok(LlmCompletion {
            text,
            model: self.config.model.clone(),
            provider: self.name().to_string(),
            usage,
        })
        .inspect(|_| {
            #[cfg(feature = "telemetry")]
            {
                // Dynamic model label is deferred: the `metrics` macro
                // requires `&'static str` for label values. Suffix the
                // counter name with the provider so Prometheus still
                // distinguishes minimax/anthropic/openai calls.
                crate::telemetry::counter!("mempalace_llm_total_minimax").increment(1);
                crate::telemetry::histogram!("mempalace_llm_latency_ms")
                    .record(_telemetry_start.elapsed().as_secs_f64() * 1000.0);
            }
            let _ = ();
        })
    }

    fn name(&self) -> &'static str {
        "minimax"
    }

    fn model(&self) -> &str {
        &self.config.model
    }

    async fn check_available(&self) -> Result<(), String> {
        if self.config.api_key.is_empty() {
            return Err("MINIMAX_API_KEY not set".to_string());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
struct MinimaxRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    messages: &'a [MinimaxMessage<'a>],
}

#[derive(serde::Serialize)]
struct MinimaxMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(serde::Deserialize)]
struct MinimaxResponse {
    content: Vec<MinimaxContent>,
    #[serde(default)]
    usage: Option<MinimaxUsage>,
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(serde::Deserialize)]
struct MinimaxContent {
    #[serde(rename = "type")]
    kind: String,
    text: String,
}

#[derive(serde::Deserialize)]
struct MinimaxUsage {
    input_tokens: u32,
    output_tokens: u32,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default_values() {
        // When MINIMAX_API_KEY is not set, default config should have empty api_key
        let config = MinimaxConfig::default();
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
        assert_eq!(config.model, DEFAULT_MODEL);
        assert_eq!(config.max_tokens, DEFAULT_MAX_TOKENS);
        assert_eq!(config.timeout, Duration::from_secs(60));
    }

    #[test]
    fn test_request_shape() {
        let req = MinimaxRequest {
            model: "MiniMax-M2.7",
            max_tokens: 800,
            system: "you are helpful",
            messages: &[MinimaxMessage {
                role: "user",
                content: "hello",
            }],
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""model":"MiniMax-M2.7""#));
        assert!(json.contains(r#""max_tokens":800"#));
        assert!(json.contains(r#""system":"you are helpful""#));
        assert!(json.contains(r#""role":"user""#));
        assert!(json.contains(r#""content":"hello""#));
    }

    #[test]
    fn test_response_parse_text() {
        let json = serde_json::json!({
            "content": [{"type": "text", "text": "hi"}],
            "usage": {"input_tokens": 3, "output_tokens": 1},
            "stop_reason": "end_turn"
        });
        let resp: MinimaxResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.content.len(), 1);
        assert_eq!(resp.content[0].text, "hi");
        assert_eq!(resp.content[0].kind, "text");
        assert!(resp.usage.is_some());
        assert_eq!(resp.usage.as_ref().unwrap().input_tokens, 3);
        assert_eq!(resp.usage.as_ref().unwrap().output_tokens, 1);
    }

    #[test]
    fn test_max_tokens_cap_800() {
        let provider = MinimaxProvider::new(MinimaxConfig {
            api_key: "test".to_string(),
            base_url: DEFAULT_BASE_URL.to_string(),
            model: DEFAULT_MODEL.to_string(),
            max_tokens: 9999, // way over the cap
            timeout: Duration::from_secs(60),
        })
        .unwrap();
        // Internally clamped to 800
        assert_eq!(provider.max_tokens_for_request(), HARD_CAP_MAX_TOKENS);
    }

    #[test]
    fn test_error_on_missing_api_key() {
        match MinimaxProvider::from_env() {
            Err(LlmError::MissingApiKey { provider }) => {
                assert_eq!(provider, "minimax");
            }
            _ => panic!("expected MissingApiKey error"),
        }
    }

    #[test]
    fn test_headers_no_sdk() {
        // Verify the API version header is correct (no x-stainless-* additions)
        assert_eq!(MINIMAX_API_VERSION, "2023-06-01");
    }

    #[test]
    fn test_response_missing_optional_fields() {
        // Response might not always include usage or stop_reason
        let json = serde_json::json!({
            "content": [{"type": "text", "text": "hello"}]
        });
        let resp: MinimaxResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.content.len(), 1);
        assert!(resp.usage.is_none());
        assert!(resp.stop_reason.is_none());
    }
}