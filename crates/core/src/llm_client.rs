//! llm_client.rs — Provider abstraction for LLM-assisted entity refinement.
//!
//! Three providers cover the useful space:
//!
//! - `ollama` (default): local models via http://localhost:11434. Works fully
//!   offline. Honors MemPalace's "zero-API required" principle.
//! - `openai-compat`: any OpenAI-compatible `/v1/chat/completions` endpoint.
//!   Covers OpenRouter, LM Studio, llama.cpp server, vLLM, Groq, Fireworks,
//!   Together, and most self-hosted setups.
//! - `anthropic`: the official Messages API. Opt-in for users who want Haiku
//!   quality without setting up a local model.
//!
//! All providers expose the same `classify(system, user, json_mode)` method and
//! the same `check_available()` probe.

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    pub text: String,
    pub model: String,
    pub provider: String,
    pub raw: serde_json::Value,
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("LLM returned non-OK: {code} — {message}")]
    NonOk { code: u16, message: String },

    #[error("Invalid JSON response: {0}")]
    InvalidJson(serde_json::Error),

    #[error("Empty response from {provider} (model={model})")]
    Empty { provider: String, model: String },

    #[error("Unexpected response shape: {0}")]
    Shape(String),

    #[error("Cannot reach {endpoint}: {reason}")]
    Unreachable { endpoint: String, reason: String },

    #[error("Unknown provider '{name}'. Choices: {choices}")]
    UnknownProvider { name: String, choices: String },
}

// ---------------------------------------------------------------------------
// Local endpoint detection (issue #24 — privacy warning support)
// ---------------------------------------------------------------------------

const LOCALHOST_HOSTS: &[&str] = &["localhost", "127.0.0.1", "::1"];

/// Return true if `url`'s hostname is on the user's machine or private network.
///
/// Local includes:
///   - localhost, 127.0.0.1, ::1
///   - hostnames ending in .local (mDNS/Bonjour)
///   - IPv4 RFC1918: 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
///   - IPv4 CGNAT (Tailscale): 100.64.0.0/10
///   - IPv6 unique-local addresses (fc00::/7)
fn extract_host(url_str: &str) -> Option<String> {
    let without_scheme = url_str
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let host = without_scheme.split(':').next().unwrap_or("");
    let host = host.trim_start_matches('[').trim_end_matches(']');
    Some(host.to_string())
}

fn endpoint_is_local(url: &Option<String>) -> bool {
    let Some(url) = url else {
        return true; // No endpoint = local (defensive)
    };
    let host = match extract_host(url) {
        Some(h) => h.to_lowercase(),
        None => return true, // Unparseable = local (defensive)
    };

    if host.is_empty() {
        return true;
    }

    if LOCALHOST_HOSTS.contains(&host.as_str()) {
        return true;
    }
    if host.ends_with(".local") {
        return true;
    }
    if host.starts_with("10.") {
        return true;
    }
    if host.starts_with("192.168.") {
        return true;
    }
    if host.starts_with("172.") {
        let parts: Vec<&str> = host.split('.').collect();
        if parts.len() >= 2 {
            if let Ok(second) = parts[1].parse::<u8>() {
                if (16..=31).contains(&second) {
                    return true;
                }
            }
        }
    }
    if host.starts_with("100.") {
        let parts: Vec<&str> = host.split('.').collect();
        if parts.len() >= 2 {
            if let Ok(second) = parts[1].parse::<u8>() {
                if (64..=127).contains(&second) {
                    return true;
                }
            }
        }
    }
    if host.starts_with("fc") || host.starts_with("fd") {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Provider trait
// ---------------------------------------------------------------------------

/// Result of availability check.
pub type Availability = (bool, String);

/// LLM provider interface. All providers must implement classify() and check_available().
pub trait LlmProvider: Send + Sync {
    /// Classify a prompt using the LLM. Returns structured text response.
    fn classify(&self, system: &str, user: &str, json_mode: bool) -> Result<LlmResponse, LlmError>;

    /// Fast availability probe. Returns (ok, message).
    fn check_available(&self) -> Availability;

    /// Provider name (e.g., "ollama", "openai-compat", "anthropic").
    fn name(&self) -> &str;

    /// Model name.
    fn model(&self) -> &str;

    /// Whether this provider sends content off the local machine.
    fn is_external_service(&self) -> bool {
        !endpoint_is_local(&self.endpoint())
    }

    /// Endpoint URL (for is_external_service heuristic).
    fn endpoint(&self) -> Option<String>;
}

// ---------------------------------------------------------------------------
// HTTP helper
// ---------------------------------------------------------------------------

fn http_post_json(
    url: &str,
    body: serde_json::Value,
    headers: &[(String, String)],
    timeout: Duration,
) -> Result<serde_json::Value, LlmError> {
    let client = Client::builder()
        .timeout(timeout)
        .build()
        .map_err(LlmError::Http)?;

    let mut req = client.post(url);
    req = req.header("Content-Type", "application/json");
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    req = req.body(body.to_string());

    let resp = req.send()?;

    let status = resp.status();
    if !status.is_success() {
        let detail = resp.text().unwrap_or_default();
        return Err(LlmError::NonOk {
            code: status.as_u16(),
            message: if detail.len() > 500 {
                detail[..500].to_string()
            } else {
                detail
            },
        });
    }

    let text = resp.text().map_err(LlmError::Http)?.trim().to_string();
    let json: serde_json::Value = serde_json::from_str(&text).map_err(LlmError::InvalidJson)?;
    Ok(json)
}

// ---------------------------------------------------------------------------
// Ollama provider
// ---------------------------------------------------------------------------

pub struct OllamaProvider {
    model: String,
    endpoint: String,
    timeout: Duration,
}

impl OllamaProvider {
    pub fn new(model: String, endpoint: Option<String>, timeout: Duration) -> Self {
        Self {
            model,
            endpoint: endpoint.unwrap_or_else(|| "http://localhost:11434".to_string()),
            timeout,
        }
    }
}

impl LlmProvider for OllamaProvider {
    fn classify(&self, system: &str, user: &str, json_mode: bool) -> Result<LlmResponse, LlmError> {
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "stream": false,
            "options": {"temperature": 0.1},
        });
        if json_mode {
            body["format"] = serde_json::json!("json");
        }

        let data = http_post_json(
            &format!("{}/api/chat", self.endpoint),
            body,
            &[],
            self.timeout,
        )?;

        let text = data
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        if text.is_empty() {
            return Err(LlmError::Empty {
                provider: self.name().to_string(),
                model: self.model.clone(),
            });
        }

        Ok(LlmResponse {
            text,
            model: self.model.clone(),
            provider: self.name().to_string(),
            raw: data,
        })
    }

    fn check_available(&self) -> Availability {
        let client = match Client::builder().timeout(Duration::from_secs(5)).build() {
            Ok(c) => c,
            Err(e) => return (false, format!("Cannot build HTTP client: {e}")),
        };

        let url = format!("{}/api/tags", self.endpoint);
        let Ok(resp) = client.get(&url).send() else {
            return (false, format!("Cannot reach Ollama at {}", self.endpoint));
        };

        let Ok(data) = resp.json::<serde_json::Value>() else {
            return (
                false,
                format!("Invalid JSON from Ollama at {}", self.endpoint),
            );
        };

        let names: std::collections::HashSet<String> = data
            .get("models")
            .and_then(|m| m.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        m.get("name")
                            .and_then(|n| n.as_str().map(|s| s.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();

        let wanted: std::collections::HashSet<String> =
            [self.model.clone(), format!("{}:latest", self.model)]
                .into_iter()
                .collect();

        if names.intersection(&wanted).count() == 0 {
            return (
                false,
                format!(
                    "Model '{}' not loaded in Ollama. Run: ollama pull {}",
                    self.model, self.model
                ),
            );
        }

        (true, "ok".to_string())
    }

    fn name(&self) -> &str {
        "ollama"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn endpoint(&self) -> Option<String> {
        Some(self.endpoint.clone())
    }
}

// ---------------------------------------------------------------------------
// OpenAI-compatible provider
// ---------------------------------------------------------------------------

pub struct OpenAICompatProvider {
    model: String,
    endpoint: Option<String>,
    api_key: Option<String>,
    timeout: Duration,
}

impl OpenAICompatProvider {
    pub fn new(
        model: String,
        endpoint: Option<String>,
        api_key: Option<String>,
        timeout: Duration,
    ) -> Self {
        Self {
            model,
            endpoint,
            api_key,
            timeout,
        }
    }

    fn resolve_url(&self) -> Result<String, LlmError> {
        let endpoint = self
            .endpoint
            .as_ref()
            .ok_or_else(|| LlmError::Unreachable {
                endpoint: "(none)".to_string(),
                reason: "no --llm-endpoint configured".to_string(),
            })?;

        let url = endpoint.trim_end_matches('/');
        let url = url.trim_end_matches("/chat/completions");
        let url = url.trim_end_matches("/v1");
        if !url.ends_with("/v1") {
            Ok(format!("{}/v1/chat/completions", url))
        } else {
            Ok(format!("{}/chat/completions", url))
        }
    }
}

impl LlmProvider for OpenAICompatProvider {
    fn classify(&self, system: &str, user: &str, json_mode: bool) -> Result<LlmResponse, LlmError> {
        let url = self.resolve_url()?;

        let mut body = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "temperature": 0.1,
        });
        if json_mode {
            body["response_format"] = serde_json::json!({"type": "json_object"});
        }

        let mut headers = Vec::new();
        if let Some(ref key) = self.api_key {
            headers.push(("Authorization".to_string(), format!("Bearer {}", key)));
        }

        let data = http_post_json(&url, body, &headers, self.timeout)?;

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
                model: self.model.clone(),
            });
        }

        Ok(LlmResponse {
            text,
            model: self.model.clone(),
            provider: self.name().to_string(),
            raw: data,
        })
    }

    fn check_available(&self) -> Availability {
        let Some(ref endpoint) = self.endpoint else {
            return (false, "no --llm-endpoint configured".to_string());
        };

        let base = endpoint
            .trim_end_matches('/')
            .trim_end_matches("/chat/completions")
            .trim_end_matches("/v1");

        let client = match Client::builder().timeout(Duration::from_secs(5)).build() {
            Ok(c) => c,
            Err(e) => return (false, format!("Cannot build HTTP client: {e}")),
        };

        let url = format!("{}/v1/models", base);
        let mut req = client.get(&url);
        if let Some(ref key) = self.api_key {
            req = req.header("Authorization", format!("Bearer {}", key));
        }

        match req.send() {
            Ok(resp) if resp.status().is_success() => (true, "ok".to_string()),
            Ok(resp) => (
                false,
                format!(
                    "HTTP {} from {}: {}",
                    resp.status().as_u16(),
                    endpoint,
                    resp.text().unwrap_or_default()
                ),
            ),
            Err(e) => (false, format!("Cannot reach {}: {}", endpoint, e)),
        }
    }

    fn name(&self) -> &str {
        "openai-compat"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn endpoint(&self) -> Option<String> {
        self.endpoint.clone()
    }
}

// ---------------------------------------------------------------------------
// Anthropic provider
// ---------------------------------------------------------------------------

const ANTHROPIC_API_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    model: String,
    endpoint: String,
    api_key: Option<String>,
    timeout: Duration,
}

impl AnthropicProvider {
    pub fn new(model: String, api_key: Option<String>, timeout: Duration) -> Self {
        Self {
            model,
            endpoint: "https://api.anthropic.com".to_string(),
            api_key,
            timeout,
        }
    }
}

impl LlmProvider for AnthropicProvider {
    fn classify(&self, system: &str, user: &str, json_mode: bool) -> Result<LlmResponse, LlmError> {
        let api_key = self.api_key.as_ref().ok_or_else(|| LlmError::Unreachable {
            endpoint: self.endpoint.clone(),
            reason: "ANTHROPIC_API_KEY not set (use --llm-api-key or env)".to_string(),
        })?;

        let mut sys_prompt = system.to_string();
        if json_mode {
            sys_prompt += "\n\nRespond with valid JSON only, no prose.";
        }

        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 2048,
            "temperature": 0.1,
            "system": sys_prompt,
            "messages": [{"role": "user", "content": user}],
        });

        let headers = [
            ("X-API-Key".to_string(), api_key.clone()),
            (
                "anthropic-version".to_string(),
                ANTHROPIC_API_VERSION.to_string(),
            ),
        ];

        let data = http_post_json(
            &format!("{}/v1/messages", self.endpoint),
            body,
            &headers,
            self.timeout,
        )?;

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
                model: self.model.clone(),
            });
        }

        Ok(LlmResponse {
            text,
            model: self.model.clone(),
            provider: self.name().to_string(),
            raw: data,
        })
    }

    fn check_available(&self) -> Availability {
        if self.api_key.is_none() {
            return (
                false,
                "ANTHROPIC_API_KEY not set (use --llm-api-key or env)".to_string(),
            );
        }
        // Don't probe — a live request would cost money. First real call will surface auth errors.
        (true, "ok".to_string())
    }

    fn name(&self) -> &str {
        "anthropic"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn endpoint(&self) -> Option<String> {
        Some(self.endpoint.clone())
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Build a provider by name. Raises LlmError on unknown provider.
pub fn get_provider(
    name: &str,
    model: &str,
    endpoint: Option<String>,
    api_key: Option<String>,
    timeout_secs: u64,
) -> Result<Box<dyn LlmProvider>, LlmError> {
    let timeout = Duration::from_secs(timeout_secs);

    match name {
        "ollama" => Ok(Box::new(OllamaProvider::new(
            model.to_string(),
            endpoint,
            timeout,
        ))),
        "openai-compat" => Ok(Box::new(OpenAICompatProvider::new(
            model.to_string(),
            endpoint,
            api_key,
            timeout,
        ))),
        "anthropic" => Ok(Box::new(AnthropicProvider::new(
            model.to_string(),
            api_key,
            timeout,
        ))),
        _ => Err(LlmError::UnknownProvider {
            name: name.to_string(),
            choices: "ollama, openai-compat, anthropic".to_string(),
        }),
    }
}

/// Get the default model for a provider.
pub fn default_model(name: &str) -> &'static str {
    match name {
        "ollama" => "gemma4:e4b",
        "openai-compat" => "gpt-4o-mini",
        "anthropic" => "claude-haiku-4-20250514",
        _ => "gemma4:e4b",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_localhost_is_local() {
        assert!(endpoint_is_local(&Some(
            "http://localhost:11434".to_string()
        )));
        assert!(endpoint_is_local(&Some(
            "http://127.0.0.1:11434".to_string()
        )));
        assert!(endpoint_is_local(&Some("http://[::1]:11434".to_string())));
    }

    #[test]
    fn test_rfc1918_is_local() {
        assert!(endpoint_is_local(&Some(
            "http://192.168.1.100:11434".to_string()
        )));
        assert!(endpoint_is_local(&Some(
            "http://10.0.0.1:11434".to_string()
        )));
        assert!(endpoint_is_local(&Some(
            "http://172.16.0.1:11434".to_string()
        )));
        assert!(endpoint_is_local(&Some(
            "http://172.31.255.255:11434".to_string()
        )));
    }

    #[test]
    fn test_public_is_not_local() {
        assert!(!endpoint_is_local(&Some(
            "https://api.anthropic.com".to_string()
        )));
        assert!(!endpoint_is_local(&Some(
            "https://api.openai.com/v1".to_string()
        )));
        assert!(!endpoint_is_local(&Some(
            "https://openrouter.ai".to_string()
        )));
    }

    #[test]
    fn test_tailscale_cgnat_is_local() {
        assert!(endpoint_is_local(&Some(
            "http://100.64.0.1:11434".to_string()
        )));
        assert!(endpoint_is_local(&Some(
            "http://100.127.255.255:11434".to_string()
        )));
        assert!(!endpoint_is_local(&Some(
            "http://100.128.0.0:11434".to_string()
        )));
    }

    #[test]
    fn test_none_endpoint_is_local() {
        assert!(endpoint_is_local(&None));
    }
}
