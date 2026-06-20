//! OpenAI-compatible remote embedding provider (G2 / mempalace parity).
//!
//! Talks to any `/embeddings` endpoint that follows the OpenAI schema —
//! OpenAI, Azure OpenAI, OpenRouter, LM Studio, vLLM, Ollama. Selected via
//! `MEMPALACE_EMBED_MODEL=openai-3-small` (and friends) behind the
//! `embed-openai` feature.
//!
//! Config (env):
//!   * `OPENAI_API_KEY`              — bearer token (required)
//!   * `OPENAI_BASE_URL`             — default `https://api.openai.com/v1`
//!   * `OPENAI_EMBEDDING_DIMENSIONS` — required only for models not in the
//!     known-dimension table below.

use super::Embedder;
use async_trait::async_trait;
use std::time::Duration;

/// Known `(model, dim)` pairs so callers don't need `OPENAI_EMBEDDING_DIMENSIONS`.
fn known_dim(model: &str) -> Option<usize> {
    match model {
        "text-embedding-3-small" | "text-embedding-ada-002" => Some(1536),
        "text-embedding-3-large" => Some(3072),
        _ => None,
    }
}

/// Remote embedder for OpenAI-compatible `/embeddings` endpoints.
pub struct OpenAIRemoteEmbedder {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    dim: usize,
    fingerprint: String,
}

impl OpenAIRemoteEmbedder {
    /// Build from environment for `model` (e.g. `text-embedding-3-small`).
    /// `OPENAI_EMBEDDING_API_KEY` / `OPENAI_EMBEDDING_BASE_URL` take
    /// precedence over `OPENAI_API_KEY` / `OPENAI_BASE_URL` so embeddings
    /// can be routed to a different endpoint than LLM calls (B20).
    pub fn from_env(model: &str) -> anyhow::Result<Self> {
        let api_key = std::env::var("OPENAI_EMBEDDING_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .map_err(|_| {
                anyhow::anyhow!(
                    "OPENAI_EMBEDDING_API_KEY or OPENAI_API_KEY is required for the openai remote embedder"
                )
            })?;
        let base_url = std::env::var("OPENAI_EMBEDDING_BASE_URL")
            .or_else(|_| std::env::var("OPENAI_BASE_URL"))
            .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
        let dim = match std::env::var("OPENAI_EMBEDDING_DIMENSIONS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
        {
            Some(d) => d,
            None => known_dim(model).ok_or_else(|| {
                anyhow::anyhow!("OPENAI_EMBEDDING_DIMENSIONS required for unknown model '{model}'")
            })?,
        };
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .connect_timeout(Duration::from_secs(10))
                .build()
                .expect("reqwest Client::builder with valid timeout durations"),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model: model.to_string(),
            dim,
            fingerprint: format!("openai:{model}:{dim}"),
        })
    }

    async fn request(&self, inputs: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        let url = format!("{}/embeddings", self.base_url);
        let body = serde_json::json!({ "model": self.model, "input": inputs });
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        let json: serde_json::Value = resp.json().await?;
        let data = json
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| anyhow::anyhow!("embeddings response missing 'data' array"))?;
        let mut out = Vec::with_capacity(data.len());
        for item in data {
            let emb = item
                .get("embedding")
                .and_then(|e| e.as_array())
                .ok_or_else(|| anyhow::anyhow!("embeddings item missing 'embedding'"))?;
            out.push(
                emb.iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect(),
            );
        }
        Ok(out)
    }
}

#[async_trait]
impl Embedder for OpenAIRemoteEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        self.request(&[text])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("empty embedding response"))
    }

    async fn embed_batch(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        self.request(texts).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dims_resolve() {
        assert_eq!(known_dim("text-embedding-3-small"), Some(1536));
        assert_eq!(known_dim("text-embedding-3-large"), Some(3072));
        assert_eq!(known_dim("text-embedding-ada-002"), Some(1536));
        assert_eq!(known_dim("mystery-model"), None);
    }

    #[test]
    fn from_env_requires_api_key() {
        let _lock = crate::test_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // SAFETY: single-threaded under test_env_lock.
        unsafe {
            std::env::remove_var("OPENAI_API_KEY");
        }
        assert!(OpenAIRemoteEmbedder::from_env("text-embedding-3-small").is_err());
    }
}
