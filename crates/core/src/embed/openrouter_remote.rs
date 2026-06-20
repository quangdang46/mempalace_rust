//! OpenRouter remote embedding provider.
//!
//! OpenRouter exposes an OpenAI-compatible `/embeddings` endpoint that
//! proxies to many upstream models (OpenAI, Cohere, Voyage, etc.). We
//! talk to it as an OpenAI-shaped API but namespace the fingerprint
//! with `openrouter:<full-model>:<dim>` so manifests written through
//! OpenRouter are NEVER confused with manifests written through a
//! direct OpenAI call. Mixing the two would silently corrupt search
//! results because the two code paths can target different model
//! versions for the same short alias.
//!
//! Selected via `MEMPALACE_EMBED_MODEL=openrouter-3-small` (and friends)
//! behind the `embed-openrouter` feature.
//!
//! Config (env):
//!   * `OPENROUTER_API_KEY` — bearer token (required)
//!
//! Endpoint: `https://openrouter.ai/api/v1/embeddings`
//!
//! Request body: `{ "input": ["<text>"], "model": "<model>" }`
//! Response:     `{ "data": [{ "embedding": [...], "index": 0 }, ...] }`

use super::Embedder;
use async_trait::async_trait;
use std::time::Duration;

/// Known `(model, dim)` pairs so callers don't need to pass a
/// dimension override. OpenRouter proxies many upstream models; the
/// dimensions listed here match the upstream providers' published
/// defaults at the time of writing.
fn known_dim(model: &str) -> Option<usize> {
    match model {
        "openai/text-embedding-3-small" | "openai/text-embedding-ada-002" => Some(1536),
        "openai/text-embedding-3-large" => Some(3072),
        "cohere/embed-english-v3.0" => Some(1024),
        "voyage/voyage-3" => Some(1024),
        _ => None,
    }
}

/// Remote embedder for OpenRouter's OpenAI-compatible `/embeddings`.
///
/// The fingerprint is `openrouter:<full-model>:<dim>`. The full model
/// string is intentionally preserved (including the `vendor/` prefix
/// that OpenRouter uses) so a manifest written by
/// `openrouter/openai/text-embedding-3-small` is distinguishable from
/// one written by `openai/text-embedding-3-small` via direct OpenAI.
/// This prevents accidentally mixing vectors between the two providers.
pub struct OpenRouterRemoteEmbedder {
    client: reqwest::Client,
    api_key: String,
    model: String,
    dim: usize,
    fingerprint: String,
}

impl OpenRouterRemoteEmbedder {
    const ENDPOINT: &'static str = "https://openrouter.ai/api/v1/embeddings";

    /// Build from environment for `model` (e.g. `openai/text-embedding-3-small`).
    ///
    /// Returns an error when `OPENROUTER_API_KEY` is unset or the
    /// `model` is not in the known-dimension table. OpenRouter does
    /// not currently expose a `dimensions` parameter, so the dim must
    /// be known up front.
    pub fn from_env(model: &str) -> anyhow::Result<Self> {
        let api_key = std::env::var("OPENROUTER_API_KEY").map_err(|_| {
            anyhow::anyhow!("OPENROUTER_API_KEY is required for the openrouter remote embedder")
        })?;
        let dim = known_dim(model).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown openrouter model '{model}'; expected one of the entries in \
                 openrouter_remote::known_dim (e.g. openai/text-embedding-3-small)"
            )
        })?;
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .connect_timeout(Duration::from_secs(10))
                .build()
                .unwrap(),
            api_key,
            model: model.to_string(),
            dim,
            // CRITICAL: include the full model string so vectors from
            // openrouter/... and direct openai/... never share a manifest.
            fingerprint: format!("openrouter:{model}:{dim}"),
        })
    }

    async fn request(&self, inputs: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        let body = serde_json::json!({ "model": self.model, "input": inputs });
        let resp = self
            .client
            .post(Self::ENDPOINT)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            // OpenRouter-specific status codes we want to surface with
            // actionable messages: 401 (bad/missing key), 402 (payment
            // required — OpenRouter charges per token, common failure
            // mode), 429 (rate limited), 5xx (upstream errors).
            let code = status.as_u16();
            let snippet = resp.text().await.unwrap_or_default();
            let snippet = snippet.chars().take(512).collect::<String>();
            return Err(match code {
                401 => anyhow::anyhow!("openrouter 401 unauthorized: {snippet}"),
                402 => anyhow::anyhow!(
                    "openrouter 402 payment required: {snippet} \
                     — top up your OpenRouter account or check your credit balance"
                ),
                429 => anyhow::anyhow!("openrouter 429 rate limited: {snippet}"),
                500..=599 => anyhow::anyhow!("openrouter {code} server error: {snippet}"),
                _ => anyhow::anyhow!("openrouter {code} unexpected status: {snippet}"),
            });
        }
        let json: serde_json::Value = resp.json().await?;
        let data = json.get("data").and_then(|d| d.as_array()).ok_or_else(|| {
            anyhow::anyhow!("openrouter embeddings response missing 'data' array")
        })?;
        let mut out = Vec::with_capacity(data.len());
        for item in data {
            let emb = item
                .get("embedding")
                .and_then(|e| e.as_array())
                .ok_or_else(|| anyhow::anyhow!("openrouter embeddings item missing 'embedding'"))?;
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
impl Embedder for OpenRouterRemoteEmbedder {
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
            .ok_or_else(|| anyhow::anyhow!("empty openrouter embedding response"))
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
        assert_eq!(known_dim("openai/text-embedding-3-small"), Some(1536));
        assert_eq!(known_dim("openai/text-embedding-3-large"), Some(3072));
        assert_eq!(known_dim("openai/text-embedding-ada-002"), Some(1536));
        assert_eq!(known_dim("cohere/embed-english-v3.0"), Some(1024));
        assert_eq!(known_dim("voyage/voyage-3"), Some(1024));
        assert_eq!(known_dim("mystery/model"), None);
    }

    #[test]
    fn from_env_requires_api_key() {
        let _lock = crate::test_env_lock().lock().unwrap();
        // SAFETY: single-threaded under test_env_lock.
        unsafe {
            std::env::remove_var("OPENROUTER_API_KEY");
        }
        assert!(
            OpenRouterRemoteEmbedder::from_env("openai/text-embedding-3-small").is_err(),
            "from_env must fail when OPENROUTER_API_KEY is unset"
        );
    }

    #[test]
    fn fingerprint_includes_full_model() {
        let _lock = crate::test_env_lock().lock().unwrap();
        // SAFETY: single-threaded under test_env_lock.
        unsafe {
            std::env::set_var("OPENROUTER_API_KEY", "test-key");
        }
        let e = OpenRouterRemoteEmbedder::from_env("openai/text-embedding-3-small").unwrap();
        // The full model string (with vendor prefix) MUST appear in
        // the fingerprint so manifests written through openrouter/
        // cannot be confused with manifests written through direct
        // openai.
        assert!(
            e.fingerprint().contains("openai/text-embedding-3-small"),
            "fingerprint must include the full model string, got: {}",
            e.fingerprint()
        );
        // And it must be namespaced under openrouter:.
        assert!(
            e.fingerprint().starts_with("openrouter:"),
            "fingerprint must start with 'openrouter:', got: {}",
            e.fingerprint()
        );
    }

    #[test]
    fn fingerprint_format() {
        let _lock = crate::test_env_lock().lock().unwrap();
        // SAFETY: single-threaded under test_env_lock.
        unsafe {
            std::env::set_var("OPENROUTER_API_KEY", "test-key");
        }
        for model in [
            "openai/text-embedding-3-small",
            "openai/text-embedding-3-large",
            "openai/text-embedding-ada-002",
            "cohere/embed-english-v3.0",
            "voyage/voyage-3",
        ] {
            let e = OpenRouterRemoteEmbedder::from_env(model).unwrap();
            let re = regex::Regex::new(r"^openrouter:[a-zA-Z0-9_./\-]+:\d+$").unwrap();
            assert!(
                re.is_match(e.fingerprint()),
                "fingerprint '{}' for model '{model}' does not match ^openrouter:[a-zA-Z0-9_./-]+:\\d+$",
                e.fingerprint()
            );
        }
    }
}
