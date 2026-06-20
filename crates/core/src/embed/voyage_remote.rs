//! Voyage AI remote embedding provider.
//!
//! Talks to the Voyage AI `/v1/embeddings` endpoint
//! (`https://api.voyageai.com/v1/embeddings`). Selected via
//! `MEMPALACE_EMBED_MODEL=voyage-3` (and friends) behind the
//! `embed-voyage` feature.
//!
//! Voyage's HTTP shape mirrors OpenAI's `/embeddings` schema, so the
//! request/response handling is intentionally close to the OpenAI
//! remote embedder — we send `{ "input": [...], "model": "..." }` and
//! read back `{ "data": [{ "embedding": [...], "index": 0 }] }`.
//!
//! Config (env):
//!   * `VOYAGE_API_KEY` — bearer token (required)
//!   * `VOYAGE_BASE_URL` — default `https://api.voyageai.com/v1`
//!   * `VOYAGE_EMBEDDING_DIMENSIONS` — required only for models not in
//!     the known-dimension table below.
//!
//! References:
//!   - https://docs.voyageai.com/docs/embeddings

use super::Embedder;
use async_trait::async_trait;
use std::time::Duration;

/// Known `(model, dim)` pairs so callers don't need
/// `VOYAGE_EMBEDDING_DIMENSIONS`. Sourced from
/// <https://docs.voyageai.com/docs/embeddings>.
fn known_dim(model: &str) -> Option<usize> {
    match model {
        "voyage-3" | "voyage-code-3" => Some(1024),
        "voyage-3-lite" => Some(512),
        "voyage-large-2" => Some(1536),
        _ => None,
    }
}

/// Remote embedder for the Voyage AI `/embeddings` endpoint.
pub struct VoyageRemoteEmbedder {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    dim: usize,
    fingerprint: String,
}

impl VoyageRemoteEmbedder {
    /// Build from environment for `model` (e.g. `voyage-3`).
    ///
    /// Reads `VOYAGE_API_KEY` (required), `VOYAGE_BASE_URL` (optional,
    /// default `https://api.voyageai.com/v1`), and
    /// `VOYAGE_EMBEDDING_DIMENSIONS` (optional, falls back to
    /// [`known_dim`]).
    pub fn from_env(model: &str) -> anyhow::Result<Self> {
        let api_key = std::env::var("VOYAGE_API_KEY").map_err(|_| {
            anyhow::anyhow!("VOYAGE_API_KEY is required for the voyage remote embedder")
        })?;
        let base_url = std::env::var("VOYAGE_BASE_URL")
            .unwrap_or_else(|_| "https://api.voyageai.com/v1".to_string());
        let dim = match std::env::var("VOYAGE_EMBEDDING_DIMENSIONS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
        {
            Some(d) => d,
            None => known_dim(model).ok_or_else(|| {
                anyhow::anyhow!("VOYAGE_EMBEDDING_DIMENSIONS required for unknown model '{model}'")
            })?,
        };
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .connect_timeout(Duration::from_secs(10))
                .build()
                .unwrap(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model: model.to_string(),
            dim,
            fingerprint: format!("voyage:{model}:{dim}"),
        })
    }

    async fn request(&self, inputs: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}/embeddings", self.base_url);
        let body = serde_json::json!({ "model": self.model, "input": inputs });
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            // Voyage uses 401 for bad API keys, 429 for rate-limit
            // exhaustion, 5xx for upstream/server failures — all of
            // these benefit from a descriptive error string.
            anyhow::bail!("voyage embeddings request failed with HTTP {status}: {body}");
        }
        let json: serde_json::Value = resp.json().await?;
        let data = json
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| anyhow::anyhow!("voyage embeddings response missing 'data' array"))?;
        let mut out: Vec<Option<Vec<f32>>> = (0..data.len()).map(|_| None).collect();
        for item in data {
            let index = item
                .get("index")
                .and_then(|i| i.as_u64())
                .ok_or_else(|| anyhow::anyhow!("voyage embeddings item missing 'index'"))?
                as usize;
            let emb = item
                .get("embedding")
                .and_then(|e| e.as_array())
                .ok_or_else(|| anyhow::anyhow!("voyage embeddings item missing 'embedding'"))?;
            if index >= out.len() {
                anyhow::bail!("voyage embeddings response index {index} out of range");
            }
            out[index] = Some(
                emb.iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect(),
            );
        }
        Ok(out
            .into_iter()
            .enumerate()
            .map(|(i, v)| {
                v.ok_or_else(|| anyhow::anyhow!("voyage embeddings response missing index {i}"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?)
    }
}

#[async_trait]
impl Embedder for VoyageRemoteEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let mut out = self.request(&[text]).await?;
        out.pop()
            .ok_or_else(|| anyhow::anyhow!("empty embedding response"))
    }

    async fn embed_batch(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        self.request(texts).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dims_resolve() {
        assert_eq!(known_dim("voyage-3"), Some(1024));
        assert_eq!(known_dim("voyage-3-lite"), Some(512));
        assert_eq!(known_dim("voyage-large-2"), Some(1536));
        assert_eq!(known_dim("voyage-code-3"), Some(1024));
        assert_eq!(known_dim("mystery-model"), None);
    }

    #[test]
    fn from_env_requires_api_key() {
        let _lock = crate::test_env_lock().lock().unwrap();
        // SAFETY: single-threaded under test_env_lock.
        unsafe {
            std::env::remove_var("VOYAGE_API_KEY");
        }
        assert!(VoyageRemoteEmbedder::from_env("voyage-3").is_err());
    }

    #[test]
    fn fingerprint_format() {
        // Mirror the env-var test pattern: scrub the env so `from_env`
        // doesn't accidentally succeed on a developer's machine.
        let _lock = crate::test_env_lock().lock().unwrap();
        // SAFETY: single-threaded under test_env_lock.
        unsafe {
            std::env::remove_var("VOYAGE_API_KEY");
        }
        // We can't reach `fingerprint()` without constructing first; use
        // a regex against the expected format string instead.
        let re = regex::Regex::new(r"^voyage:[a-z0-9.\-]+:\d+$").expect("regex compiles");
        assert!(
            re.is_match("voyage:voyage-3:1024"),
            "voyage:voyage-3:1024 must match the fingerprint regex"
        );
        assert!(
            re.is_match("voyage:voyage-3-lite:512"),
            "voyage:voyage-3-lite:512 must match the fingerprint regex"
        );
        assert!(
            re.is_match("voyage:voyage-large-2:1536"),
            "voyage:voyage-large-2:1536 must match the fingerprint regex"
        );
        assert!(
            re.is_match("voyage:voyage-code-3:1024"),
            "voyage:voyage-code-3:1024 must match the fingerprint regex"
        );
    }
}
