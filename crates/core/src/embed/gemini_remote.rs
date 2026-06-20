//! Google Gemini remote embedding provider.
//!
//! Talks to Gemini's `batchEmbedContents` endpoint using the
//! `GEMINI_API_KEY` (or `GOOGLE_API_KEY` fallback) env var. Selected
//! via `MEMPALACE_EMBED_MODEL=gemini-text-004` (and friends) behind the
//! `embed-gemini` feature.
//!
//! Config (env):
//!   * `GEMINI_API_KEY`                 — query-param key (required)
//!   * `GOOGLE_API_KEY`                 — fallback if `GEMINI_API_KEY` is unset
//!   * `GEMINI_EMBEDDING_DIMENSIONS`    — required only for models not in
//!     the known-dimension table below.
//!
//! Auth note: Gemini rejects the `Authorization: Bearer` header and
//! instead expects the API key as a `?key=...` query parameter. Do not
//! add an Authorization header — the request will be refused with 400.

use super::Embedder;
use async_trait::async_trait;
use std::time::Duration;

/// Base URL for the Gemini batch-embed endpoint (the model name and
/// `:batchEmbedContents` action are appended below).
const ENDPOINT_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";

/// Known `(model, dim)` pairs so callers don't need
/// `GEMINI_EMBEDDING_DIMENSIONS`.
fn known_dim(model: &str) -> Option<usize> {
    match model {
        "text-embedding-004" => Some(768),
        "embedding-001" => Some(768),
        _ => None,
    }
}

/// Remote embedder for Google's Gemini `batchEmbedContents` endpoint.
pub struct GeminiRemoteEmbedder {
    client: reqwest::Client,
    api_key: String,
    model: String,
    dim: usize,
    fingerprint: String,
}

impl GeminiRemoteEmbedder {
    /// Build from environment for `model` (e.g. `text-embedding-004`).
    /// Returns `None` if neither `GEMINI_API_KEY` nor `GOOGLE_API_KEY`
    /// is set, or if the model is unknown and no explicit dimensions
    /// override is configured.
    pub fn from_env(model: &str) -> Option<Self> {
        let api_key = std::env::var("GEMINI_API_KEY")
            .or_else(|_| std::env::var("GOOGLE_API_KEY"))
            .ok()?;
        let dim = match std::env::var("GEMINI_EMBEDDING_DIMENSIONS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
        {
            Some(d) => d,
            None => known_dim(model)?,
        };
        Some(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .connect_timeout(Duration::from_secs(10))
                .build()
                .expect("reqwest Client::builder with valid timeout durations"),
            api_key,
            model: model.to_string(),
            dim,
            fingerprint: format!("gemini:{model}:{dim}"),
        })
    }

    async fn request(&self, inputs: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        let url = format!(
            "{ENDPOINT_BASE}/{}:batchEmbedContents?key={}",
            self.model, self.api_key
        );
        let requests: Vec<serde_json::Value> = inputs
            .iter()
            .map(|text| {
                serde_json::json!({
                    // Note the "models/" prefix on the body field —
                    // it is *not* in the URL even though both refer
                    // to the same model identifier.
                    "model": format!("models/{}", self.model),
                    "content": { "parts": [{ "text": text }] },
                    "taskType": "RETRIEVAL_DOCUMENT",
                })
            })
            .collect();
        let body = serde_json::json!({ "requests": requests });

        let resp = self.client.post(&url).json(&body).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let kind = match status.as_u16() {
                401 | 403 => "authentication",
                429 => "rate-limit",
                500..=599 => "upstream",
                _ => "http",
            };
            anyhow::bail!("gemini {kind} error (status {status}): {text}");
        }

        let json: serde_json::Value = resp.json().await?;
        let embeddings = json
            .get("embeddings")
            .and_then(|d| d.as_array())
            .ok_or_else(|| anyhow::anyhow!("gemini response missing 'embeddings' array"))?;
        let mut out = Vec::with_capacity(embeddings.len());
        for item in embeddings {
            let values = item
                .get("values")
                .and_then(|v| v.as_array())
                .ok_or_else(|| anyhow::anyhow!("gemini embedding item missing 'values'"))?;
            out.push(
                values
                    .iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect(),
            );
        }
        Ok(out)
    }
}

#[async_trait]
impl Embedder for GeminiRemoteEmbedder {
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
            .ok_or_else(|| anyhow::anyhow!("empty gemini embedding response"))
    }

    async fn embed_batch(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        // Gemini caps each `batchEmbedContents` call at 100 requests.
        const CHUNK: usize = 100;
        let mut out = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(CHUNK) {
            let mut got = self.request(chunk).await?;
            out.append(&mut got);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dims_resolve() {
        assert_eq!(known_dim("text-embedding-004"), Some(768));
        assert_eq!(known_dim("embedding-001"), Some(768));
        assert_eq!(known_dim("mystery-model"), None);
    }

    #[test]
    fn from_env_requires_api_key() {
        let _lock = crate::test_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // SAFETY: single-threaded under test_env_lock.
        unsafe {
            std::env::remove_var("GEMINI_API_KEY");
            std::env::remove_var("GOOGLE_API_KEY");
        }
        assert!(GeminiRemoteEmbedder::from_env("text-embedding-004").is_none());
    }

    #[test]
    fn from_env_accepts_google_api_key() {
        let _lock = crate::test_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // SAFETY: single-threaded under test_env_lock.
        unsafe {
            std::env::remove_var("GEMINI_API_KEY");
            std::env::set_var("GOOGLE_API_KEY", "test-google-key");
        }
        let e = GeminiRemoteEmbedder::from_env("text-embedding-004")
            .expect("should construct from GOOGLE_API_KEY fallback");
        assert_eq!(e.dim(), 768);
        assert_eq!(e.api_key, "test-google-key");
        assert_eq!(e.fingerprint(), "gemini:text-embedding-004:768");
    }

    #[test]
    fn fingerprint_format() {
        let re = regex::Regex::new(r"^gemini:[a-z0-9.\-]+:\d+$").unwrap();
        let fp = "gemini:text-embedding-004:768";
        assert!(re.is_match(fp), "fingerprint '{fp}' did not match regex");
        // Negative cases.
        assert!(!re.is_match("openai:foo:768"));
        assert!(!re.is_match("gemini:UPPER:768"));
        assert!(!re.is_match("gemini:foo"));
        assert!(!re.is_match("gemini:foo:"));
    }
}
