//! `TractEmbedder` — pure-Rust ONNX inference via `tract-onnx` (mp-014).
//!
//! Uses `tokio::task::spawn_blocking` to do CPU-bound ONNX inference without
//! blocking the async reactor. HuggingFace `tokenizers` is used for tokenisation.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use async_trait::async_trait;
use ndarray::Array2;
use tokenizers::Tokenizer;
use tract_onnx::prelude::{tvec, Framework, TValue, Tensor};

use super::Embedder;

fn huggingface_cache_dir() -> PathBuf {
    let base = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(base)
        .join(".cache")
        .join("huggingface")
        .join("hub")
}

fn ensure_cached(
    model_name: &str,
    onnx_path: &PathBuf,
    tokenizer_path: &PathBuf,
) -> anyhow::Result<()> {
    if onnx_path.exists() && tokenizer_path.exists() {
        return Ok(());
    }
    let repo = if model_name.contains('/') {
        model_name.to_string()
    } else {
        format!("model2vec/{}", model_name)
    };
    download_from_huggingface(&repo, "model.onnx", onnx_path)?;
    download_from_huggingface(&repo, "tokenizer.json", tokenizer_path)?;
    Ok(())
}

fn download_from_huggingface(repo: &str, path: &str, dest: &PathBuf) -> anyhow::Result<()> {
    if dest.exists() {
        return Ok(());
    }
    let url = format!(
        "https://huggingface.co/{}/resolve/main/{}",
        repo,
        path.replace(' ', "%20")
    );
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("tract: create cache dir for {}", path))?;
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let response = client
        .get(&url)
        .header("User-Agent", "mempalace/1.0")
        .send()
        .with_context(|| format!("tract: download {} from HF: {}", path, url))?
        .error_for_status()
        .with_context(|| format!("tract: HF download failed for {}: {}", path, url))?;
    let bytes = response
        .bytes()
        .with_context(|| format!("tract: read body for {} from HF", path))?;
    std::fs::write(dest, bytes).with_context(|| format!("tract: write {} to cache", path))?;
    Ok(())
}

fn normalize_l2(mut v: Vec<f32>) -> Vec<f32> {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-9 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

const DEFAULT_MAX_LEN: usize = 256;

fn probe_dimension(model_path: &PathBuf) -> anyhow::Result<usize> {
    let inference_model = tract_onnx::onnx()
        .model_for_path(model_path)
        .context("tract: load for probe")?;
    let runnable = inference_model
        .into_runnable()
        .context("tract: build runnable for probe")?;

    // Token IDs as f32 (most ONNX models expect float inputs for token IDs)
    let input_ids: Vec<f32> = vec![1.0, 2.0, 3.0];
    let input_tensor: Tensor = Array2::from_shape_vec((1, 3), input_ids)
        .map_err(|e| anyhow::anyhow!("tract: probe shape: {}", e))?
        .into();
    let input_tvalue: TValue = input_tensor.into();
    let result = runnable
        .run(tvec!(input_tvalue))
        .map_err(|e| anyhow::anyhow!("tract: probe run: {}", e))?;
    let out = result
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("tract: no probe output"))?;
    let view = out
        .to_array_view::<f32>()
        .map_err(|e| anyhow::anyhow!("tract: probe view: {}", e))?;
    let hidden = *view.shape().last().unwrap_or(&384);
    if hidden == 0 {
        anyhow::bail!("tract: probe returned zero-dimensional output");
    }
    Ok(hidden)
}

pub struct TractEmbedder {
    /// Opaque handle — the concrete type is confined to `run_embed_batch`.
    _model_handle: Arc<()>,
    tokenizer_path: PathBuf,
    dim: usize,
    fingerprint: String,
}

impl TractEmbedder {
    pub fn with_model(
        model_name: impl Into<String>,
        _cache_dir: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        let model_name_owned = model_name.into();

        let cache = huggingface_cache_dir();
        let model_path = cache.join(&model_name_owned).join("model.onnx");
        let tokenizer_path = cache.join(&model_name_owned).join("tokenizer.json");

        if !model_path.exists() || !tokenizer_path.exists() {
            ensure_cached(&model_name_owned, &model_path, &tokenizer_path)?;
        }

        let dim = probe_dimension(&model_path)?;
        let fp = format!("tract:{}:{}", model_name_owned, dim);

        Ok(Self {
            _model_handle: Arc::new(()),
            tokenizer_path,
            dim,
            fingerprint: fp,
        })
    }
}

#[async_trait]
impl Embedder for TractEmbedder {
    fn dim(&self) -> usize {
        self.dim
    }
    fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        let mut out = self.embed_batch(&[text]).await?;
        out.pop()
            .ok_or_else(|| anyhow::anyhow!("embed: empty batch returned from embedder"))
    
    }

    async fn embed_batch(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let tokenizer_path = self.tokenizer_path.clone();
        let model_name = tokenizer_path
            .parent()
            .map(|p| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_default();

        let owned: Vec<String> = texts.iter().map(|s| (*s).to_owned()).collect();
        let dim = self.dim;

        tokio::task::spawn_blocking(move || {
            run_embed_batch(&model_name, tokenizer_path.as_path(), &owned, dim)
        })
        .await
        .map_err(|e| anyhow::anyhow!("tract: spawn_blocking join error: {}", e))
        .and_then(|r| r)
        .context("tract: embed failed")
    }
}

fn run_embed_batch(
    model_name: &str,
    tokenizer_path: &Path,
    texts: &[String],
    dim: usize,
) -> anyhow::Result<Vec<Vec<f32>>> {
    let cache = huggingface_cache_dir();
    let model_path = cache.join(model_name).join("model.onnx");

    let inference_model = tract_onnx::onnx()
        .model_for_path(&model_path)
        .with_context(|| format!("tract: failed to load ONNX model '{}'", model_name))?;

    let runnable = inference_model
        .into_runnable()
        .with_context(|| format!("tract: failed to build runnable for '{}'", model_name))?;

    let tokenizer = Tokenizer::from_file(tokenizer_path.to_str().unwrap())
        .map_err(|e| anyhow::anyhow!("tract: failed to load tokenizer: {}", e))?;

    let encodings: Vec<_> = texts
        .iter()
        .map(|s| {
            tokenizer
                .encode(s.as_str(), true)
                .map_err(|e| anyhow::anyhow!("tract: tokenize: {}", e))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let max_len = encodings
        .iter()
        .map(|e| e.get_ids().len())
        .max()
        .unwrap_or(1)
        .min(DEFAULT_MAX_LEN);
    let batch_size = encodings.len();
    let mut input_ids = vec![0f32; batch_size * max_len];
    let mut attention_mask = vec![0f32; batch_size * max_len];

    for (i, enc) in encodings.iter().enumerate() {
        let ids = enc.get_ids();
        let len = ids.len().min(max_len);
        for (j, &id) in ids.iter().take(len).enumerate() {
            input_ids[i * max_len + j] = id as f32;
            attention_mask[i * max_len + j] = 1.0;
        }
    }

    let input_ids_tensor: Tensor = Array2::from_shape_vec((batch_size, max_len), input_ids)
        .map_err(|e| anyhow::anyhow!("tract: input_ids: {}", e))?
        .into();
    let attention_mask_tensor: Tensor =
        Array2::from_shape_vec((batch_size, max_len), attention_mask)
            .map_err(|e| anyhow::anyhow!("tract: attention_mask: {}", e))?
            .into();
    let input_tvalue: TValue = input_ids_tensor.into();
    let mask_tvalue: TValue = attention_mask_tensor.into();

    let result = runnable.run(tvec!(input_tvalue, mask_tvalue))?;

    let output = result
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("tract: no output tensor"))?;

    let view = output
        .to_array_view::<f32>()
        .map_err(|e| anyhow::anyhow!("tract: output view: {}", e))?;

    let shape = view.shape();
    if shape.len() != 3 {
        anyhow::bail!(
            "tract: expected 3D output [batch, seq, dim], got {:?}",
            shape
        );
    }

    let seq_len = shape[1];
    let mut embeddings = Vec::with_capacity(batch_size);

    for (i, enc) in encodings.iter().enumerate() {
        let valid_len = enc.get_ids().len().min(seq_len).min(max_len);
        let mut sum = vec![0f32; dim];
        let mut count = 0f32;
        for j in 0..valid_len {
            for k in 0..dim {
                sum[k] += view[[i, j, k]];
            }
            count += 1.0;
        }
        if count > 0.0 {
            for x in &mut sum {
                *x /= count;
            }
        }
        embeddings.push(normalize_l2(sum));
    }

    Ok(embeddings)
}
