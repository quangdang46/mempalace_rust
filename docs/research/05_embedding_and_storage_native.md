# Native Embeddings & Vector Storage for `mempalace_rust`

> Research report — **does not modify code**.
> Author: orchestrated `research_embedding_and_storage_options` session.
> Date: 2026-05-25.
> Scope: replace the Python ONNX subprocess and re-evaluate `embedvec` for the upcoming jcode integration.

---

## 0. State of the world (audit)

Before recommending replacements, a precise picture of *what is in the binary today* matters because some of the constraints inherited from the Python port turn out not to actually bind the Rust runtime.

Files inspected:

- `crates/core/Cargo.toml` — declares `embedvec = "0.5"` and `edgebert = "0.4"`.
- `crates/core/src/onnx_embed.rs` — spawns `python3 -u onnx_embed_python.py --persistent`, talks newline-delimited JSON over stdio.
- `crates/core/src/onnx_embed_python.py` — imports `chromadb.utils.embedding_functions.onnx_mini_lm_l6_v2.ONNXMiniLM_L6_V2` (384-dim, all-MiniLM-L6-v2).
- `crates/core/src/palace_db.rs` — defines two backends:
  - `PalaceDb` — JSON HashMap, scored by `naive_similarity` (Jaccard word-overlap, **no embeddings**).
  - `EmbeddingDb` — wraps `embedvec::HnswIndex` + `embedvec::VectorStorage` + `OnnxModel`.
- `crates/core/src/searcher.rs` / `miner.rs` / `mcp_server.rs` — every live caller goes through `PalaceDb`. `EmbeddingDb` has zero call-sites outside `palace_db.rs` itself.
- `edgebert` is in `[dependencies]` but `grep edgebert::` returns no hits.

Conclusion the report builds on:

| Claim | Reality |
|---|---|
| "We use ONNX embeddings + HNSW for retrieval" | **False.** The production path scores Jaccard word-overlap over a JSON HashMap. |
| "Python is required for embeddings" | True only for the dead-code `EmbeddingDb` and the bench harness. The mainline runtime never embeds anything. |
| "Switching the embedder is the hard part" | False. Switching the embedder is *cheap* because nothing currently depends on it. The expensive part is **wiring vectors into the search path**, which `embedvec` was supposed to do but never got hooked up. |
| `embedvec` does the heavy lifting today | Wrong, it sits unused. There is no persistence, no payload filtering, no deletion semantics tied to it in production code. |

This changes the migration strategy materially: we are not preserving a working embedding pipeline — we are **building the first one** while simultaneously deleting the Python dependency. That gives a lot of design freedom and removes the usual "don't regress recall" backstop. The benchmark numbers in the README (96.6% LongMemEval R@5) come from the *Python* implementation; the Rust port's mainline retrieval is currently word-overlap and is not benchmarked at parity.

---

## Part A — Embedding inference

All numbers below are CPU-only unless noted, on a modern x86_64 laptop class machine, batch size 1 unless noted, sequence length 256, FP32, all-MiniLM-L6-v2 as the reference workload (because that is the current default and the comparison axis with jcode). Latencies are typical published / measured ranges from project READMEs and benchmark posts; they should be treated as order-of-magnitude unless re-measured locally.

### A.1 Candidate matrix

| Crate | Backing runtime | Build complexity | Binary size impact | Runtime memory (MiniLM-L6) | Single-vec latency (CPU, MiniLM-L6, seq≈64) | Batch-32 latency | GPU | License |
|---|---|---|---|---|---|---|---|---|
| **`ort` 2.x** (onnxruntime crate) | C++ ONNX Runtime, dynamic-link or download-binary | Medium. `download-binaries` feature pulls a prebuilt `libonnxruntime.so` (~12 MB) at build/run; `load-dynamic` uses system lib. CMake/clang not required. | +12–20 MB on disk for the ORT shared lib (separate file, not statically linked). | ~80–110 MB resident with model loaded. | 4–8 ms | 25–60 ms (≈1 ms/seq amortized) | Yes — feature flags `cuda`, `directml`, `coreml`, `tensorrt`, `rocm`, `webgpu`. CPU-only build is the default. | MIT/Apache-2.0 (crate); MIT (ORT) |
| **`fastembed-rs` 4.x** | uses `ort` + `tokenizers` | Low. One `cargo add fastembed`. Auto-downloads model + tokenizer from HF on first call into a cache dir. Same ORT shared lib as `ort`. | Same as `ort` (+12–20 MB ORT) plus model files cached at runtime (~22 MB MiniLM-L6, ~133 MB BGE-base, ~330 MB BGE-large per model). | ~90–130 MB resident for MiniLM-L6; scales with model. | 5–10 ms | 25–70 ms | Inherited from `ort` (CUDA/DML behind feature). | Apache-2.0 |
| **`model2vec-rs` 0.x** | static word-embedding lookup, no transformer at inference time | Very low. Pure Rust, no native dep, no ONNX. Loads a small distilled `.safetensors`/JSON. | +5–10 MB for the crate; model files 8–30 MB depending on choice. | ~20–40 MB resident. | <1 ms (often 50–200 µs) | <2 ms | N/A — already CPU-trivial. | MIT |
| **`candle-core` + `candle-transformers` 0.8.x** | pure Rust ML framework (HF) | Low–Medium. Pure Rust, builds clean; sentence-transformers config requires hand-wiring tokenizer + pooling. CUDA/Metal feature flags are well-supported. | +6–12 MB Rust crates. No native lib. | ~120–180 MB resident; mmap of safetensors helps for larger models. | 8–18 ms | 60–140 ms | Yes — `cuda`, `metal`, `accelerate` feature flags. | MIT/Apache-2.0 |
| **`burn` 0.15.x** | pluggable backends (NdArray, WGPU, LibTorch, Candle) | Medium–High. Excellent code quality but the “sentence-transformers in burn” path requires writing the model graph yourself or using community examples. No drop-in MiniLM. | +8–15 MB depending on backend. | Comparable to candle. | 10–25 ms (NdArray backend) | 80–200 ms | Yes via WGPU/CUDA/LibTorch backends. | MIT/Apache-2.0 |
| **`rust-bert` 0.23.x** | wraps `tch` (libtorch) | High. Requires libtorch (~500 MB download), specific PyTorch version pinning, glibc compatibility, painful on Windows. | +500 MB libtorch on disk (not statically linkable). | ~250 MB resident minimum. | 6–12 ms | 30–80 ms | Yes (CUDA libtorch). | Apache-2.0 |
| **`mistral.rs` (`mistralrs-core`)** | custom inference engine focused on autoregressive LLMs; embedding endpoint is recent | High. Heavy dep tree, designed for full LLM serving; embedding mode supported but overkill. | +30–50 MB build artifacts; per-model weights. | ≥300 MB even for small models. | 15–40 ms | 80–250 ms | Yes (CUDA/Metal). | MIT |
| **`tch-rs` 0.18** | libtorch C++ | High. Same libtorch baggage as rust-bert; you implement the model. | +500 MB libtorch. | ~250 MB minimum. | similar to rust-bert | similar | Yes. | Apache-2.0 |
| **`tract` (`tract-onnx` 0.21)** *(used by jcode)* | pure-Rust ONNX execution | Low. No native dep at all. Pure Rust. Slightly slower than ORT. Limited operator coverage but covers BERT/MiniLM cleanly. | +8–14 MB Rust deps, no shared lib. | ~80–110 MB resident. | 8–14 ms | 50–110 ms | None (CPU only). | MIT/Apache-2.0 |

### A.2 Model coverage

| Crate | MiniLM-L6 | MiniLM-L12 | multilingual-MiniLM-L12 | BGE-small | BGE-base | BGE-large | E5-small/base | Jina v2 | Nomic-embed-text | gte-small |
|---|---|---|---|---|---|---|---|---|---|---|
| `ort` (any `.onnx`) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `fastembed-rs` (curated) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| `model2vec-rs` | distilled static variants only (potion-base-2m / 8m, M2V variants) | — | distilled multilingual variants | distilled BGE variants | — | — | — | — | distilled | — |
| `candle-transformers` | ✅ (BERT) | ✅ | ✅ | ✅ | ✅ | ✅ (with effort) | ✅ | ⚠ partial | ⚠ partial | ✅ |
| `burn` | ⚠ DIY | ⚠ DIY | ⚠ DIY | ⚠ DIY | ⚠ DIY | ⚠ DIY | ⚠ DIY | ⚠ DIY | ⚠ DIY | ⚠ DIY |
| `rust-bert` | ✅ | ✅ | ✅ | ⚠ via custom config | ⚠ | — | ⚠ | — | — | — |
| `mistral.rs` | ❌ (focus on causal LMs) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ (recent) | ❌ |
| `tch-rs` raw | ⚠ DIY | ⚠ DIY | ⚠ DIY | ⚠ DIY | ⚠ DIY | ⚠ DIY | ⚠ DIY | ⚠ DIY | ⚠ DIY | ⚠ DIY |
| `tract` (jcode path) | ✅ | ✅ | ✅ (model has BERT ops) | ✅ | ⚠ heavy | ⚠ heavy | ✅ | ⚠ may hit unsupported ops | ⚠ may hit unsupported ops | ✅ |

Notes:

- `fastembed-rs` curates a list of "supported models" with tested tokenizer + pooling. As of 4.x that includes all-MiniLM-L6/L12, multilingual MiniLM, BGE small/base/large (en + zh + multi), E5 small/base/large, GTE small/base, Jina v2 small/base, Nomic-embed-text-v1/v1.5 (with the Matryoshka tail), and several reranker models. Adding a new model is a small PR (config in `models.rs`).
- `tract` is the most conservative pure-Rust path. It handles MiniLM cleanly. BGE-large and Nomic occasionally use ops `tract` does not implement; jcode's pinned model is MiniLM-L6 specifically because that combo is known-good.
- `model2vec-rs` is qualitatively different: static word vectors with a smart pooling, ~50× faster than transformer embeddings, lossy on retrieval quality (LongMemEval scores typically drop 5–15 absolute points vs MiniLM-L6, but at sub-millisecond latency).

### A.3 jcode-embedding cross-read

`/data/projects/jcode/crates/jcode-embedding/src/lib.rs` (and the identical copy in `jcode-multi-agent`):

- Uses `tract-hir` + `tract-onnx` 0.21 for inference.
- Uses `tokenizers` 0.21 (HuggingFace, pure Rust, `default-features=false, features=["onig"]`).
- Hardcodes `all-MiniLM-L6-v2` (`MODEL_NAME`, `EMBEDDING_DIM=384`, `MAX_SEQ_LENGTH=256`).
- Downloads `model.onnx` and `tokenizer.json` from `huggingface.co/sentence-transformers/all-MiniLM-L6-v2` on first use via `reqwest::blocking`.
- Mean-pools over valid tokens, then L2-normalizes — semantically equivalent to what `sentence-transformers` does for that model.
- Public surface: `Embedder::load_from_dir`, `embed`, `embed_batch`, free fns `cosine_similarity`, `batch_cosine_similarity`, `find_similar`, `is_model_available`, `embedding_dim()`.

This is small, vendored, dependency-light, and produces the same 384-dim vectors mempalace expects.

### A.4 Recommendation — embedder

**Default embedder: `fastembed-rs` (which uses `ort`).**

Reasoning:

1. **Coverage.** The user-listed candidate set (BGE-small/large, E5, Jina, Nomic, MiniLM-L6/L12, multilingual-MiniLM, gte-small) is exactly fastembed's curated list. Adding new models is an upstream PR, not a refactor.
2. **Speed.** `ort` (C++ ONNX Runtime) is meaningfully faster than `tract` for transformer ops (1.5–3× depending on model and CPU). For the jcode integration target ("inserts every turn, search every turn"), throughput matters.
3. **Auto-download.** fastembed bakes in HF Hub download with a configurable cache dir, avoiding the need to re-implement what jcode-embedding does (the `download_model_blocking` block is ~30 LOC of `reqwest::blocking`).
4. **GPU optionality.** Adding `ort` features `cuda` / `coreml` / `directml` is one feature-flag away if a power user wants it. Local-first remains the default.
5. **Maturity.** fastembed-rs ships in production at several vector-DB vendors (Qdrant, etc.) and tracks ORT releases.

The cost: a `libonnxruntime.so` (or `.dylib` / `.dll`) shipped alongside the `mpr` binary. That is the only break with the current "single binary" promise, and it is mitigated either by the `download-binaries` feature (one-shot fetch on first run) or by static-linking ORT in CI for releases.

**Lightweight default: `model2vec-rs`.**

For hardware-constrained users (Raspberry Pi, $5/mo VPS, embedded MacBook Air with thermal throttling), `model2vec-rs` is the right choice. Sub-millisecond inference, ~30 MB total footprint, no native lib. Quality drop is real but acceptable for L0/L1/wake-up paths where retrieval is not the bottleneck.

**Fallback / portability path: `candle-transformers`.**

If `ort` ever becomes a packaging headache (uncommon platforms, sandboxed builds, single-static-binary requirement), `candle` is the pure-Rust escape hatch that keeps coverage of the model list. Slower than `ort` (2–3×) but builds clean everywhere `cargo` does.

**Why not `tract` like jcode?** It is a fine pure-Rust path and exactly what we'd recommend if the project were already shipping `tract`. But:

- It is materially slower than `ort` on transformer workloads.
- Op coverage is narrower; switching from MiniLM to BGE-large or Nomic in the future would risk hitting unsupported ops.
- We do not gain much over `fastembed-rs` for the same default model — fastembed already wraps the HF download and pooling logic that `jcode-embedding` had to write inline.

The right place for `tract` is *if and only if* mempalace must run inside jcode's process and we choose option (B) below.

### A.5 Reuse vs trait — the jcode question

Two viable patterns:

**(A) Mempalace exposes a trait, host injects an embedder.**

```rust
// In mempalace-core (illustrative — do not implement here)
pub trait Embedder: Send + Sync {
    fn dim(&self) -> usize;
    fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>>;
    fn embed_batch(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>>;
}
```

`mempalace-core` ships a default `FastEmbedEmbedder`. When run as a CLI/MCP server, the default is used. When run *as a library inside jcode*, jcode injects its existing `jcode_embedding::Embedder` (wrapped in a thin adapter), so:

- Only one ORT/tract instance is loaded per process.
- Only one model is downloaded into `~/.cache/...`.
- Both crates retrieve the same 384-dim vector space, so a memory written by mempalace and a query run by jcode are commensurable.

**(B) Mempalace re-uses `jcode-embedding` directly when the `jcode-host` feature is on.**

`mempalace-core` adds a Cargo feature `jcode-host` that pulls `jcode-embedding` as a dependency and uses it as the default embedder. Without that feature, `fastembed-rs` is used.

**Recommendation: do (A), with (B) as a thin convenience.**

(A) is the right primitive — it lets *any* host (jcode, an OpenClaw skill, a Hermes plugin, a future jupyter kernel, a third-party app) inject its own embedder without taking on `fastembed-rs` or `ort` as a transitive dep. That is also the only way to satisfy users who want to use a remote embedding API (OpenAI `text-embedding-3-large`, Cohere, Voyage), which is a pattern already requested in upstream Python (issue #756).

(B) is then a 30-line adapter shipped behind `--features jcode-host`:

```rust
#[cfg(feature = "jcode-host")]
impl Embedder for jcode_embedding::Embedder {
    fn dim(&self) -> usize { jcode_embedding::embedding_dim() }
    fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> { self.embed(text) }
    fn embed_batch(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> { self.embed_batch(texts) }
}
```

That gives jcode a one-liner integration without forcing every other consumer to take on `tract` or `ort`.

A cross-cutting consequence: **the trait contract must include `dim()`** so the storage layer can validate at construction time. Today's `OnnxModel::dimension()` returns a hardcoded 384 and never validates — that footgun should not survive into the new design.

### A.6 Feature-flag strategy for "small/big" choice

```toml
# crates/core/Cargo.toml — illustrative, not applied
[features]
default = ["embed-fastembed"]
embed-fastembed = ["dep:fastembed"]              # Tier-A default (BGE-class, multilingual)
embed-model2vec = ["dep:model2vec-rs"]           # Tier-B: low-power machines
embed-candle    = ["dep:candle-core", "dep:candle-transformers"]  # Tier-A pure-Rust
embed-tract     = ["dep:tract-hir", "dep:tract-onnx", "dep:tokenizers"]  # parity with jcode
embed-remote    = ["dep:reqwest"]                # OpenAI / Cohere / Voyage / Ollama / vLLM
jcode-host      = ["dep:jcode-embedding"]        # zero-copy reuse when running inside jcode
```

Pick exactly one of `embed-*` per build. Document in `mpr doctor` which one is active and what dim it produces. CI matrix builds at minimum: `embed-fastembed` (Linux/macOS/Windows), `embed-model2vec` (Linux), `embed-candle` (Linux). The `embed-remote` and `jcode-host` features only get smoke-tested.

Runtime selection of model (within a given embedder) stays in `MEMPALACE_EMBED_MODEL` / config, exactly as today — for fastembed this maps to `EmbeddingModel::*` enum values; for model2vec it maps to a HF repo id. The feature flag picks the *runtime*, not the *model file*.

---

## Part B — Vector storage

### B.1 Where `embedvec` actually lives in the repo

Recap from the audit: `embedvec` is in `Cargo.toml` and `EmbeddingDb` wraps it, but no live code path constructs an `EmbeddingDb`. The mainline storage today is "JSON HashMap + Jaccard". So the comparison below is against (a) the *intended* `embedvec` design and (b) the *actual* JSON-HashMap status quo.

`embedvec` itself: in-memory HNSW + flat vector storage, persistence via manual `bincode` save/load, no payload filter language, no async, no transactions. Single-writer fork-unsafe. Fine for thousands; gets expensive past ~100k due to in-memory full-state assumption.

### B.2 Candidate matrix

| Backend | Persistence | Payload filtering | Deletion cost | Max-vectors tested in published benchmarks | Approx. on-disk per 384-dim vector + small payload | Fork safety / multi-writer | Async API | License |
|---|---|---|---|---|---|---|---|---|
| `embedvec` (current) | Manual bincode dump; HNSW rebuilt on load | Pre-filter by callback only | Tombstone + occasional rebuild; O(N) in worst case | ~10⁵ (community reports) | ~1.7 KB (vector) + JSON metadata | No (single-process, in-memory) | No | MIT |
| `lancedb` 0.10+ | Columnar Lance files (versioned, ACID-ish) on disk | SQL-like `WHERE` over columns | Logical delete + compaction; cheap if compaction is regular | 10⁸+ (production at Roboflow, Character.ai) | ~1.5 KB (Arrow + zstd) for vec+payload | Yes — multi-writer with versioned commits | Yes (`tokio`) | Apache-2.0 |
| `qdrant` (Rust client + embedded) | RocksDB segments + WAL on disk | Rich JSON payload filter language | Logical delete + segment compaction; cheap | 10⁹ (production) | ~2.0 KB | Yes — embedded server, multi-process via gRPC | Yes (gRPC) | Apache-2.0 |
| `surrealdb` 2.x with vector index | Custom KV (RocksDB / TiKV / mem) + WAL | Full SurrealQL graph + vector | Logical delete; depends on backend | ~10⁶ in published demos | ~3.0 KB (richer schema) | Yes — embedded or server | Yes (`tokio`) | BSL/Apache-2.0 (mixed) |
| `usearch` 2.x (Rust binding) | mmap'd single index file | None native — sidecar payload store needed | Logical delete; rebuild for true purge | 10⁸+ (paper benchmarks) | ~1.6 KB (index only; payload separate) | mmap is single-writer | No (sync) | Apache-2.0 |
| `instant-distance` 0.6 | bincode snapshot | None | Rebuild on delete | ≤10⁴ comfortably | ~1.7 KB (pure vec, no payload) | No | No | Apache-2.0 |
| `hnsw_rs` 0.3 | bincode snapshot | None | Rebuild on delete | ~10⁵–10⁶ | ~1.7 KB (pure vec) | No | No | MIT |
| `oasysdb` 0.7 | RocksDB-style files | Tag filter | Logical delete + vacuum | ~10⁶ in public demos | ~2.0 KB | Embedded single-process | No | Apache-2.0 |
| `arroy` 0.6 (Meilisearch) | LMDB transactions | None native (use sidecar) | LMDB delete is cheap | 10⁷+ (Meilisearch production) | ~1.6 KB (index) | Multi-reader / single-writer (LMDB) | No | MIT |
| `tantivy` 0.22 (BM25 / lexical) | Segment directory on disk | Faceted filter language | Logical delete, segment merges | 10⁸+ (production) | ~1.0 KB per doc (text only, no vector) | Yes (single-writer/multi-reader) | No (sync, but cheap) | MIT |
| Raw `redb` / `sled` + custom HNSW | Embedded ACID KV | Whatever you build | Whatever you build | bounded by RAM for HNSW | ~1.7 KB + KV overhead | redb single-writer; sled multi | Mixed | Apache-2.0 / Apache-2.0 |

Notes / caveats:

- "On-disk per vector" assumes 384 floats + a ~50–200 byte payload (id, source, wing, room, mtime). FP16 quantization halves the vector cost; PQ/SQ further. None of the backends are turning that on by default.
- "Async API" means **truly** async — i.e. the call-site can `.await` without a blocking-pool detour. `lancedb` and `qdrant` are the only ones that genuinely qualify; everything else either is sync or is sync wrapped in `spawn_blocking`.
- `tantivy` is here because the README explicitly couples BM25 reranking with vector search — `tantivy` would replace the in-memory `bm25.rs` scorer with a real lexical index for the 30% BM25 weight in the hybrid score.
- `surrealdb`'s license recently shifted to BSL; the Apache part covers the embedded engine. Re-check at adoption time if license matters for distribution.

### B.3 Tier recommendations for jcode integration

Inserts/searches every turn means the storage layer is on the hot path, but palace size is heavily user-dependent. Three tiers cleanly map to three backends:

#### Tier 1 — small palace (in-memory + flush)
**Target: <5,000 drawers. New project, fresh user, new wing.**
**Pick: `hnsw_rs` (or keep `embedvec` if PR-cost is too high).**

- All vectors fit in <20 MB RAM. HNSW build is sub-second.
- Persistence: single bincode/postcard file, atomic rename on flush.
- Inserts buffered to RAM, flushed on `drop` or every N seconds.
- Payload sits in a sibling SQLite (we already have `rusqlite` in the workspace via `knowledge_graph.rs` — reuse it).
- No async story needed; latency is sub-millisecond. Even at 5k drawers a brute-force cosine on 384-dim vectors finishes in ~3 ms, so HNSW is a nice-to-have here.

If we want zero migration, **`embedvec` itself is fine for Tier 1** — its weakness is scale, not correctness.

#### Tier 2 — medium palace (persistent HNSW)
**Target: 5,000 – 100,000 drawers. The "heavy individual user" or small team.**
**Pick: `usearch` (Rust binding) or `arroy`.**

- `usearch`: mmap'd index file, near-instant cold start, true incremental insert. The downside is no built-in payload — pair with SQLite for `wing`/`room`/`source_file`/timestamps. The pre-filter problem (we want to filter to a wing first, then ANN-search) is solvable by maintaining one usearch index per wing and a global index for cross-wing queries; total memory still <500 MB at 100k drawers.
- `arroy`: LMDB-backed. Slightly slower inserts but the LMDB transaction story is excellent for the "agent edits palace and reads it 50 ms later" pattern. Good fit if a multi-process model becomes relevant.

Either is a strict upgrade over `embedvec` at this size. I'd default to `usearch` because the binary search and tunable index parameters (M, ef_construction, ef_search) are easier to expose to power users.

#### Tier 3 — huge palace (columnar / Qdrant-class)
**Target: 100,000+ drawers. Power user mining 5 years of conversations, or a small team palace.**
**Pick: `lancedb` (preferred) or embedded `qdrant`.**

- `lancedb` wins on three axes:
  - True async API in tokio.
  - SQL-like payload filtering (`WHERE wing = ? AND room = ?`) without bolt-on.
  - Versioned columnar storage — incremental backup is `cp -r`, time-travel is built in, which fits MemPalace's "verbatim, never lose data" promise.
  - Apache-2.0, easy to vendor, no separate server process.
- `qdrant` embedded: more battle-tested at the 10⁸+ scale, richer payload language, but the embedded mode is less polished than lancedb and forces a gRPC layer even for in-process use.

`surrealdb` would also work, but its ambitions are wider than ours and the dependency graph is heavy. We are not building a graph DB at this layer (we already have `knowledge_graph.rs` doing temporal triples on SQLite).

### B.4 BM25 / lexical complement

The README's hybrid-search story (70% vector + 30% BM25) currently uses `crates/core/src/bm25.rs`, an in-memory recompute of TF/IDF over the candidate set. That is fine for a top-50 candidate list but loses the actual benefit of BM25 — corpus-level statistics. At Tier 2/3 scale, swapping in `tantivy` for the lexical leg makes the hybrid score honest:

- Candidate generation: vector top-K from Tier 2/3 store + BM25 top-K from `tantivy`.
- Merge by reciprocal rank fusion or weighted sum.
- Both stores honor the same `wing` / `room` filter columns.

For Tier 1, the current in-memory BM25 stays; it's not worth the complexity.

### B.5 Tier transition mechanics

Important: tiers should be **invisible to callers** behind a single `PalaceStore` trait. The storage struct picks its tier at open time based on `palace_size_hint` (count of existing drawers, cheap to read from a manifest) and a config override. Auto-promotion (Tier 1 → 2 → 3 as the palace grows) is done by a background `mpr repair compact` step, never silently mid-run.

---

## Part C — Combined recommendation

### C.1 Target architecture

```
                            +-----------------------+
                            |  trait Embedder       |
                            |   .dim()              |
                            |   .embed()            |
                            |   .embed_batch()      |
                            +----------^------------+
                                       |
        +------------------------------+----------------------------+
        |                              |                            |
+---------------+              +----------------+         +-------------------+
| FastEmbed     |              | Model2Vec      |         | Jcode (adapter)   |
| (default)     |              | (small/cheap)  |         | feat=jcode-host   |
+---------------+              +----------------+         +-------------------+

                            +-----------------------+
                            |  trait PalaceStore    |
                            |   .upsert()           |
                            |   .delete()           |
                            |   .query()            |
                            |   .filter()           |
                            +----------^------------+
                                       |
        +------------------------------+----------------------------+
        |                              |                            |
+---------------+              +----------------+         +-------------------+
| Tier 1: hnsw  |              | Tier 2: usearch|         | Tier 3: lancedb   |
| + sqlite      |              | + sqlite       |         | (single backend)  |
+---------------+              +----------------+         +-------------------+
```

Two orthogonal traits, picked independently. `mempalace-core` ships a single concrete `Palace` struct that owns one `Box<dyn Embedder>` and one `Box<dyn PalaceStore>` and exposes the public API.

### C.2 Migration path — incremental, PR-sized

The audit found that the current production path (`PalaceDb` + Jaccard) does not actually use the embedder at all. That makes step ordering easier than usual: we can build the new vector path *next to* the existing Jaccard path, validate it against the bench harness, and only then flip the default.

Each step below is meant to be a single PR, independently shippable, independently revertable, with green CI on all three OSes.

#### PR 1 — `Embedder` trait + null implementation
- Add `crates/core/src/embed/mod.rs` with the trait shown in §A.5.
- Add `NullEmbedder` (returns zero-vector, dim configurable) so existing code paths compile without an embedder.
- No behavior change. `EmbeddingDb` and `OnnxModel` stay; `EmbeddingDb::new` now takes `Box<dyn Embedder>` instead of constructing `OnnxModel` directly.
- Tests: trait dyn-compat, null embedder dim round-trip.
- **Ships**: refactor only.

#### PR 2 — `FastEmbedEmbedder` behind `embed-fastembed` feature
- Add `fastembed = "4"` and the adapter.
- Wire `mpr doctor` to print the active embedder + dim.
- `MEMPALACE_EMBED_MODEL` env var maps to the fastembed model enum (with a small helper).
- Default feature on. CI builds both `--no-default-features --features embed-fastembed` and `--no-default-features --features embed-tract` (see PR 4) to prove the trait is honest.
- **Ships**: new optional embedder; nothing currently calls it.

#### PR 3 — Delete the Python ONNX subprocess
- Remove `onnx_embed.rs`, `onnx_embed_python.py`, the `__pycache__` dir, and the `python3` requirement from `install.sh` / `install.ps1`.
- Replace `OnnxModel::load()` call sites (only the dead `EmbeddingDb` and the `crates/bench` harness) with the new trait. The bench crate gets the same `embed-fastembed` feature.
- **Ships**: removes Python from runtime requirements. README's "Requirements" section shrinks. Single-binary promise restored.

#### PR 4 — `embed-tract` parity feature + `jcode-host` adapter
- Add an alternative `embed-tract` feature (uses `tract-onnx` + `tokenizers`, basically inlined `jcode-embedding`) for users who want the pure-Rust path.
- Add a `jcode-host` feature that depends on `jcode-embedding` and provides the adapter.
- Cargo CI: matrix the three embedder features.
- **Ships**: choice of embedder runtimes; jcode integration unblocked.

#### PR 5 — `PalaceStore` trait + extract current behavior
- Define the trait in `crates/core/src/store/mod.rs`.
- Refactor `PalaceDb` to implement it. The Jaccard implementation becomes `JaccardJsonStore` and is renamed accordingly (`PalaceDb` becomes a thin wrapper that holds a `Box<dyn PalaceStore>`).
- All call sites (`searcher.rs`, `miner.rs`, `mcp_server.rs`, `layers.rs`, `sweeper.rs`, etc.) move from concrete `PalaceDb` to the trait via the wrapper. No semantic change.
- Tests: trait compliance suite (insert N, query, filter, delete, count, get_all).
- **Ships**: refactor only, no user-visible change.

#### PR 6 — Tier 1 vector store (`HnswSqliteStore`)
- New backend implementing `PalaceStore`: `hnsw_rs` for the index, SQLite for payload.
- Vector path: `Embedder::embed` → `hnsw.insert(id, vec)` → `sqlite.upsert(id, payload)`.
- Query path: `Embedder::embed(query)` → `hnsw.search(top_k * 3)` → SQLite `IN`-fetch payload → wing/room post-filter → BM25 rerank using existing `bm25.rs`.
- Opt-in: `mpr init --store hnsw` or env var. Default still Jaccard until PR 8.
- Bench harness comparisons: Jaccard vs HNSW on a 5k-drawer fixture, LongMemEval-style task, document recall@5/10.
- **Ships**: vector search exists for the first time. Users can opt in.

#### PR 7 — Tier 2 store (`UsearchSqliteStore`) and Tier 3 store (`LancedbStore`)
- Two separate sub-PRs, in either order.
- Same trait, different concrete type. Each has its own integration test fixture and `cargo bench` numbers.
- A new `mpr doctor` check reports which tier is active and recommends promotion if drawer count crosses thresholds.
- **Ships**: scale path for power users.

#### PR 8 — Flip the default
- The Tier 1 `HnswSqliteStore` becomes the default in `mpr init`.
- Existing JSON palaces are auto-migrated on first open via a one-shot `mpr migrate` step (re-embed every drawer; flush). The Jaccard `JaccardJsonStore` stays available behind `--store legacy-jaccard` for users who want zero embedding cost.
- README: replace "naive_similarity" claims with the actual vector search behavior; line up the LongMemEval claim with reproduced Rust numbers.
- **Ships**: the headline change, gated behind the prior six PRs.

#### PR 9 (optional, follow-up) — Tantivy lexical leg
- Replace in-memory `bm25.rs` with `tantivy` for Tier 2/3.
- Hybrid search now fuses two real indexes instead of one real + one ad-hoc.
- **Ships**: better recall on keyword-heavy queries, especially at 100k+.

### C.3 Risk register

| Risk | Mitigation |
|---|---|
| ORT shared lib breaks single-binary promise | `embed-tract` feature is the pure-Rust escape hatch; `model2vec-rs` for tiny machines. Document trade-off in install.sh. |
| FastEmbed model download fails in air-gapped env | Allow pre-staging model files via `MEMPALACE_EMBED_MODEL_PATH`. Mirror jcode-embedding's "load_from_dir" pattern. |
| Auto-migration from JSON to vector store is slow on huge legacy palaces | Migration is per-drawer, resumable, and can run in the background; add `--migrate-resume`. |
| jcode and mempalace use different default models, vectors are not commensurable | Trait contract requires `dim()` validation; mismatched dims fail loud at open time, not on first query. |
| Tier 3 (lancedb) adds Apache-arrow toolchain weight | Tier 3 only loads when explicitly opted in or auto-promoted — Tier 1/2 users do not pay the cost. |
| `surrealdb` BSL ambiguity | We are not recommending surrealdb, so this risk is moot. |
| Trait-object cost on hot path (`Box<dyn Embedder>`) | Negligible compared to embedder forward-pass cost (5–10 ms); benchmarked, not assumed. |

### C.4 Out-of-scope (explicitly)

- Replacing `knowledge_graph.rs` (SQLite temporal triples) is unrelated to this report.
- Multi-tenancy / multi-process palace access. Tier 3 (lancedb) makes that more achievable, but the scope is "single user, single process" today.
- Quantization (FP16, PQ, SQ). Worth a separate report once Tier 2 is shipping.
- Reranker models (cross-encoder reranking after vector search). The current "Haiku rerank" path goes through the LLM client, not the embedder; that stays as is.

---

## Appendix — Quick decision crib

- **Embedder default**: `fastembed-rs` (uses `ort`) at default model `BGEsmallEnV15` for English, `multilingual-e5-small` when locale is non-en.
- **Embedder fallback for tiny machines**: `model2vec-rs` with `potion-base-8M`.
- **Embedder for jcode in-process**: trait + `jcode-host` feature, reuses `jcode_embedding::Embedder`.
- **Storage Tier 1 (≤5k)**: `hnsw_rs` + `rusqlite`.
- **Storage Tier 2 (5k–100k)**: `usearch` + `rusqlite` (or `arroy` if multi-reader matters).
- **Storage Tier 3 (≥100k)**: `lancedb`.
- **Lexical leg (optional)**: keep in-memory `bm25.rs` for Tier 1; `tantivy` for Tier 2/3.
- **Migration**: nine PR-sized steps; the Python subprocess can be deleted in PR 3 because nothing in the live retrieval path depends on its output today.
