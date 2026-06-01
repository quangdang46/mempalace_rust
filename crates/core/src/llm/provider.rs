//! LLM provider trait — async abstraction for text completion and image description.
//!
//! 1:1 mapping to agentmemory's LLM provider interface, adapted for Rust async.

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
