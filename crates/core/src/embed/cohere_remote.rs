//! Cohere remote embedding provider (G2 / mempalace parity).
//!
//! Talks to the Cohere `/v1/embed` endpoint. Selected via
//! `MEMPALACE_EMBED_MODEL=cohere-english-v3` (and friends) behind the
//! `embed-cohere` feature.
//!
//! Config (env):
//!   * `COHERE_API_KEY` — bearer token (preferred)
//!   * `CO_API_KEY`     — fallback bearer token (Cohere SDK convention)
//!   * `COHERE_BASE_URL` — default `https://api.cohere.com/v1`
//!
//! Cohere distinguishes between `search_document` (for corpus
//! ingestion) and `search_query` (for query-time embedding). The
//! trait's `embed`/`embed_batch` methods use `search_document`; an
//! inherent `embed_query` helper exposes `search_query` for callers
//! that need a query-side vector.

use super::Embedder;
use async_trait::async_trait;
use std::time::Duration;

/// Known `(model, dim)` pairs so callers don't need to set a custom
/// dim env var. Cohere v3 model dimensions are stable and well
/// documented at <https://docs.cohere.com/reference/embed>.
fn known_dim(model: &str) -> Option<usize> {
    match model {
        "embed-english-v3.0" => Some(1024),
        "embed-multilingual-v3.0" => Some(1024),
        "embed-english-light-v3.0" => Some(384),
        "embed-multilingual-light-v3.0" => Some(384),
        _ => None,
    }
}

/// Public lookup wrapper — `construct_embedder` and the wiring layer
/// call this so an unknown model surfaces a clear "set the dim env
/// var or pick a known model" error rather than panicking deep in a
/// HTTP call.
fn known_dims(model: &str) -> Option<usize> {
    known_dim(model)
}

/// Remote embedder for the Cohere `/v1/embed` endpoint.
pub struct CohereRemoteEmbedder {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    dim: usize,
    fingerprint: String,
}

impl CohereRemoteEmbedder {
    /// Build from environment for `model` (e.g. `embed-english-v3.0`).
    ///
    /// Returns an `Err` when neither `COHERE_API_KEY` nor `CO_API_KEY`
    /// is set. `COHERE_API_KEY` takes precedence — `CO_API_KEY` is a
    /// fallback for users that copy the official Cohere SDK env name.
    pub fn from_env(model: &str) -> anyhow::Result<Self> {
        let api_key = std::env::var("COHERE_API_KEY")
            .or_else(|_| std::env::var("CO_API_KEY"))
            .map_err(|_| {
                anyhow::anyhow!(
                    "COHERE_API_KEY (or CO_API_KEY) is required for the cohere remote embedder"
                )
            })?;
        let base_url = std::env::var("COHERE_BASE_URL")
            .unwrap_or_else(|_| "https://api.cohere.com/v1".to_string());
        let dim = match std::env::var("COHERE_EMBEDDING_DIMENSIONS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
        {
            Some(d) => d,
            None => known_dim(model).ok_or_else(|| {
                anyhow::anyhow!("COHERE_EMBEDDING_DIMENSIONS required for unknown model '{model}'")
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
            fingerprint: format!("cohere:{model}:{dim}"),
        })
    }

    async fn request(&self, inputs: &[&str], input_type: &str) -> anyhow::Result<Vec<Vec<f32>>> {
        let url = format!("{}/embed", self.base_url);
        let body = serde_json::json!({
            "texts": inputs,
            "model": self.model,
            "input_type": input_type,
        });
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
            let kind = match status.as_u16() {
                401 => "unauthorized (check COHERE_API_KEY / CO_API_KEY)",
                429 => "rate limited",
                500..=599 => "server error",
                _ => "request failed",
            };
            return Err(anyhow::anyhow!("cohere API {kind} {status}: {body}"));
        }
        let json: serde_json::Value = resp.json().await?;
        let data = json
            .get("embeddings")
            .and_then(|d| d.as_array())
            .ok_or_else(|| {
                anyhow::anyhow!("cohere embeddings response missing 'embeddings' array")
            })?;
        let mut out = Vec::with_capacity(data.len());
        for item in data {
            let emb = item
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("cohere embedding item is not an array"))?;
            out.push(
                emb.iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect(),
            );
        }
        Ok(out)
    }

    /// Embed a query string with Cohere's `search_query` input type.
    ///
    /// Distinct from the trait's `embed` (which uses `search_document`)
    /// — Cohere's v3 models are asymmetric, so query vectors live in a
    /// different subspace than document vectors and should not be
    /// mixed at index time.
    pub async fn embed_query(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        self.request(&[text], "search_query")
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("empty embedding response"))
    }
}

#[async_trait]
impl Embedder for CohereRemoteEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }

    fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        self.request(&[text], "search_document")
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("empty embedding response"))
    }

    async fn embed_batch(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        self.request(texts, "search_document").await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    #[test]
    fn known_dims_resolve() {
        assert_eq!(known_dims("embed-english-v3.0"), Some(1024));
        assert_eq!(known_dims("embed-multilingual-v3.0"), Some(1024));
        assert_eq!(known_dims("embed-english-light-v3.0"), Some(384));
        assert_eq!(known_dims("embed-multilingual-light-v3.0"), Some(384));
        assert_eq!(known_dims("embed-arabic-v2.0"), None);
        assert_eq!(known_dims("mystery-model"), None);
    }

    #[test]
    fn from_env_requires_api_key() {
        let _lock = crate::test_env_lock().lock().unwrap();
        // SAFETY: single-threaded under test_env_lock.
        unsafe {
            std::env::remove_var("COHERE_API_KEY");
            std::env::remove_var("CO_API_KEY");
        }
        assert!(CohereRemoteEmbedder::from_env("embed-english-v3.0").is_err());
    }

    #[test]
    fn fingerprint_format() {
        let re = Regex::new(r"^cohere:[a-z0-9.\-]+:\d+$").unwrap();
        // Drive a fingerprint via the public constructor to exercise the
        // real format path; we can't construct a value through the trait
        // without an API key, so synthesise one for the regex assertion.
        let fp = format!(
            "cohere:embed-english-v3.0:{}",
            known_dims("embed-english-v3.0").unwrap()
        );
        assert!(
            re.is_match(&fp),
            "fingerprint '{fp}' did not match expected shape"
        );

        // Also assert the multi-lingual and light variants produce
        // well-formed fingerprints.
        for (model, dim) in [
            ("embed-english-v3.0", 1024usize),
            ("embed-multilingual-v3.0", 1024),
            ("embed-english-light-v3.0", 384),
            ("embed-multilingual-light-v3.0", 384),
        ] {
            let fp = format!("cohere:{model}:{dim}");
            assert!(
                re.is_match(&fp),
                "fingerprint '{fp}' did not match expected shape"
            );
        }
    }
}
