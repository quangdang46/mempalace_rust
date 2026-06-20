## Description

Several critical bugs in `embed/` and `connect/`:

### 1. No HTTP timeouts on any remote embedder (5 files)

All 5 remote embedders create `reqwest::Client::new()` without timeouts, which can hang indefinitely:

| File | Line | Code |
|---|---|---|
| `embed/openai_remote.rs` | 63 | `Client::new()` |
| `embed/voyage_remote.rs` | 73 | `Client::new()` |
| `embed/cohere_remote.rs` | 60 | `Client::new()` |
| `embed/openrouter_remote.rs` | 63 | `Client::new()` |
| `embed/gemini_remote.rs` | 47 | `Client::new()` |

**Impact**: If any remote API hangs (network partition, upstream deadlock), the entire search/embed pipeline blocks indefinitely, cascading to MCP tool calls and CLI commands that trigger embedding.

**Fix**: Add `.timeout(Duration::from_secs(30))` and `.connect_timeout(Duration::from_secs(10))` to each `Client::builder()` call.

### 2. embed/tract.rs — ONNX model reloaded on every request

`crates/core/src/embed/tract.rs`:

- `_model_handle: Arc<()>` (line 116) stores *nothing* — it's a no-op handle
- Every `embed_batch()` call re-downloads the ONNX model if missing (line 199), re-parses the tokenizer (line 209), and rebuilds the inference runnable (line 206)
- `probe_dimension()` (lines 83-112) loads the full model just to get output dim, then `embed_batch()` loads it **again** — 2x model load cost per request

**Impact**: ~100ms-1s per embed call instead of ~10ms, 2x ONNX load per request.

**Fix**: Cache `model`, `tokenizer`, and `runnable` in struct fields. Extract dimension from the first load instead of `probe_dimension()`.

### 3. embed/fastembed.rs, model2vec.rs, tract.rs — `embed()` returns empty vec silently

All three embed `embed()` methods use `.pop().unwrap_or_default()` when `embed_batch` returns 0 results:

```rust
// fastembed.rs:140, model2vec.rs:121, tract.rs:160
let mut out = self.embed_batch(&[text]).await?;
Ok(out.pop().unwrap_or_default())  // silent empty vec
```

**Impact**: A protocol violation returns an empty `Vec<f32>` instead of an error. This corrupts downstream search (empty vectors score randomly).

### Fix details

1. **HTTP timeouts**: `reqwest::Client::builder().timeout(Duration::from_secs(30)).connect_timeout(Duration::from_secs(10)).build()?`
2. **tract caching**: Store `Option<(tract::SimplePlan<...>, Tokenizer)>` in the struct
3. **Empty vec guard**: `.ok_or_else(|| anyhow::anyhow!("embedder returned 0 results for 1 input"))?`
