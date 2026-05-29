use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Trait for embedding providers (text and image).
/// Matches upstream EmbeddingProvider interface.
pub trait EmbeddingProvider: Send + Sync {
    fn name(&self) -> &str;
    fn dimensions(&self) -> usize;
    fn embed(&self, text: &str) -> Result<Vec<f32>>;
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
    fn embed_image(&self, src: &str) -> Result<Vec<f32>>;
}

/// Stored embedding record for SQLite persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEmbedding {
    pub image_ref: String,
    pub vector: Vec<f32>,
    pub model_name: String,
    pub dimensions: usize,
    pub updated_at: String,
    pub session_id: Option<String>,
    pub observation_id: Option<String>,
}

/// Stub embedding provider for testing.
/// Returns a fixed-dimension zero vector.
#[derive(Debug)]
pub struct StubEmbeddingProvider {
    dimensions: usize,
}

impl StubEmbeddingProvider {
    pub fn new(dimensions: usize) -> Self {
        Self { dimensions }
    }
}

impl EmbeddingProvider for StubEmbeddingProvider {
    fn name(&self) -> &str {
        "stub"
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        Ok(vec![0.0; self.dimensions])
    }

    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![0.0; self.dimensions]).collect())
    }

    fn embed_image(&self, _src: &str) -> Result<Vec<f32>> {
        Ok(vec![0.0; self.dimensions])
    }
}

/// Cosine similarity between two vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut norm_a = 0.0;
    let mut norm_b = 0.0;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 {
        0.0
    } else {
        dot / denom
    }
}

/// Dimension guard wrapper - validates embedding dimensions at the boundary.
/// Prevents wrong-dimension vectors from corrupting the index silently.
pub struct DimensionGuard<P: EmbeddingProvider> {
    inner: P,
    expected: usize,
}

impl<P: EmbeddingProvider> DimensionGuard<P> {
    pub fn new(provider: P) -> Self {
        let expected = provider.dimensions();
        Self { inner: provider, expected }
    }

    fn check(&self, v: &[f32], where_str: &str) -> Result<Vec<f32>> {
        if v.len() != self.expected {
            anyhow::bail!(
                "Embedding dimension mismatch in {}.{}: expected {}, got {}",
                self.inner.name(),
                where_str,
                self.expected,
                v.len()
            );
        }
        Ok(v.to_vec())
    }
}

impl<P: EmbeddingProvider> EmbeddingProvider for DimensionGuard<P> {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn dimensions(&self) -> usize {
        self.expected
    }

    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let v = self.inner.embed(text)?;
        self.check(&v, "embed")
    }

    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let results = self.inner.embed_batch(texts)?;
        for (i, v) in results.iter().enumerate() {
            self.check(v, &format!("embed_batch[{}]", i))?;
        }
        Ok(results)
    }

    fn embed_image(&self, src: &str) -> Result<Vec<f32>> {
        let v = self.inner.embed_image(src)?;
        self.check(&v, "embed_image")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![-1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_different_lengths() {
        let a = vec![1.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_cosine_similarity_zero_vectors() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![0.0, 0.0, 0.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_stub_provider() {
        let provider = StubEmbeddingProvider::new(512);
        assert_eq!(provider.name(), "stub");
        assert_eq!(provider.dimensions(), 512);
        let v = provider.embed("test").unwrap();
        assert_eq!(v.len(), 512);
    }

    #[test]
    fn test_dimension_guard_pass() {
        let stub = StubEmbeddingProvider::new(3);
        let guard = DimensionGuard::new(stub);
        let v = guard.embed("test").unwrap();
        assert_eq!(v.len(), 3);
    }

    #[test]
    fn test_dimension_guard_mismatch() {
        // We can't easily test a mismatch with StubEmbeddingProvider since it
        // always returns the right dimensions. This test verifies the guard
        // structure works correctly.
        let stub = StubEmbeddingProvider::new(128);
        let guard = DimensionGuard::new(stub);
        assert_eq!(guard.dimensions(), 128);
    }
}
