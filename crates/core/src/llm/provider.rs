//! LLM provider trait — async abstraction for text completion and image description.
//!
//! 1:1 mapping to mempalace's LLM provider interface, adapted for Rust async.

use async_trait::async_trait;

/// Result of an LLM completion.
#[derive(Debug, Clone)]
pub struct LlmCompletion {
    pub text: String,
    pub model: String,
    pub provider: String,
    pub usage: Option<LlmUsage>,
}

/// Token usage from an LLM response.
#[derive(Debug, Clone)]
pub struct LlmUsage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
}

/// Error type for LLM operations.
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("provider returned non-OK status: {code} {message}")]
    NonOk { code: u16, message: String },

    #[error("invalid JSON response: {0}")]
    InvalidJson(#[from] serde_json::Error),

    #[error("empty response from provider={provider} model={model}")]
    Empty { provider: String, model: String },

    #[error("unexpected response shape: {0}")]
    Shape(String),

    #[error("provider {name} not found; available: {choices}")]
    UnknownProvider { name: String, choices: String },

    #[error("API key not configured for {provider}")]
    MissingApiKey { provider: String },

    /// `mr-2k4g`: API key was sourced from a process environment variable
    /// but the user has not granted consent to use it. `reason` is a
    /// short machine-readable explanation surfaced to the CLI.
    #[error("API key for {provider} requires consent: {reason}")]
    ConsentRequired { provider: String, reason: String },

    /// `mr-ktc7`: provider rejected the configured model — usually because
    /// the model name is wrong, retired, or only available on a different
    /// endpoint. The fallback chain treats this as a hard re-resolve
    /// signal: it rebuilds a fresh provider from the next factory and
    /// retries the call, rather than counting it against the circuit
    /// breaker like a normal failure.
    #[error("model {model} not found at provider {provider}")]
    ModelNotFound { provider: String, model: String },

    #[error("request timeout after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    #[error("circuit breaker open for {provider}")]
    CircuitOpen { provider: String },
}

/// Async LLM provider trait.
///
/// All providers must be `Send + Sync + 'static` for use behind `Arc`.
#[async_trait]
pub trait LlmProvider: Send + Sync + 'static {
    /// Human-readable provider name (e.g., "openai", "anthropic", "noop").
    fn name(&self) -> &str;

    /// Model identifier (e.g., "gpt-4o-mini", "claude-sonnet-4-20250514").
    fn model(&self) -> &str;

    /// Complete a chat conversation.
    ///
    /// `system` is the system prompt, `user` is the user message.
    /// Returns the assistant's response text.
    async fn complete(&self, system: &str, user: &str) -> Result<LlmCompletion, LlmError>;

    /// Describe an image.
    ///
    /// `image_base64` is the base64-encoded image data, `mime` is the MIME type,
    /// and `prompt` is the description request.
    /// Default implementation returns an error — only providers with vision support override this.
    async fn describe_image(
        &self,
        _image_base64: &str,
        _mime: &str,
        _prompt: &str,
    ) -> Result<LlmCompletion, LlmError> {
        Err(LlmError::Shape(format!(
            "describe_image not supported by {}",
            self.name()
        )))
    }

    /// Fast availability probe. Returns `true` if the provider is reachable
    /// and properly configured.
    async fn check_available(&self) -> Result<(), String>;

    /// Generate embeddings for text using this provider's embedding model.
    ///
    /// Returns a vector of f32 values representing the text.
    /// Default implementation returns an error — providers with embedding support override this.
    async fn embed_text(&self, text: &str) -> Result<Vec<f32>, LlmError> {
        Err(LlmError::Shape(format!(
            "embed_text not supported by {}",
            self.name()
        )))
    }
}

/// `mr-ktc7`: a factory closure that builds a fresh `Arc<dyn LlmProvider>`
/// on demand. The fallback chain invokes a factory when it needs to
/// re-resolve a provider (e.g. after `LlmError::ModelNotFound`), so each
/// retry sees the most current env / config snapshot — including any
/// model the user may have just exported.
pub type ProviderFactory =
    Box<dyn Fn() -> anyhow::Result<std::sync::Arc<dyn LlmProvider>> + Send + Sync>;
