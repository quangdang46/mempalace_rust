# Local Models

MemPalace works with any local LLM — Llama, Mistral, or any offline model. Since local models generally don't speak MCP yet, there are two approaches.

## Wake-Up Command

Load your world into the model's context:

```bash
mpr wake-up > context.txt
# Paste context.txt into your local model's system prompt
```

This gives your local model a bounded wake-up context, typically around **~600-900 tokens** in the current implementation. It includes:
- **Layer 0**: Your identity — who you are, what you work on
- **Layer 1**: Top moments from the palace — key decisions, recent work

For project-specific context:
```bash
mpr wake-up --wing driftwood > context.txt
```

## CLI Search

Query on demand, feed results into your prompt:

```bash
mpr search "auth decisions" > results.txt
# Include results.txt in your prompt
```

## Rust API

For programmatic integration with your local model pipeline:

```rust
use mempalace::searcher::search_memories;

let results = search_memories(
    "auth decisions",
    palace_path="~/.mempalace/palace",
    wing=None,
    room=None,
    n_results=5,
)?;

let context = results.results
    .iter()
    .map(|r| format!("[{}/{}] {}", r.wing, r.room, r.text))
    .collect::<Vec<_>>()
    .join("\n");

// Inject into your local model's prompt
let prompt = format!("Context from memory:\n{}\n\nUser: What did we decide about auth?");
```

## AAAK for Compression

Use [AAAK dialect](/concepts/aaak-dialect) to compress wake-up context further:

```bash
mpr compress --wing myapp --dry-run
```

AAAK is readable by any LLM that reads text — Claude, GPT, Gemini, Llama, Mistral — without a decoder.

## Full Offline Stack

The core memory stack can run offline:
- **embedvec** (SQLite-based vector store) on your machine — vector storage and search
- **Local model** on your machine — reasoning and responses
- **AAAK** for compression — optional, no cloud dependency
- **ONNX embeddings** — runs entirely local, no API calls

## Embedding Model Configuration

MemPalace uses ONNX embeddings by default (`ONNXMiniLM_L6_V2`, 384 dimensions). You can configure different embedding models via environment:

```bash
# Use OpenAI embeddings (requires API key)
export MEMPALACE_EMBED_MODEL=text-embedding-3-small

# Use multilingual model for non-English content
export MEMPALACE_EMBED_MODEL=paraphrase-multilingual-MiniLM-L12-v2
```

See [Configuration](/guide/configuration) for details.
