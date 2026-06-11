# LongMemEval-S Baseline — mempalace_rust v0.3.0

**Date:** 2026-06-11
**Commit:** `ccca3b2 feat(v0.3.0): full post-merge agentmemory parity — all phases implemented`
**Harness version:** `mp-003.v1`
**Embedding model:** `bge-small-en-v15` (default, used at `open_with_embedder` time)

## Results

| Metric | Rust v0.3.0 | Phase 0 lexical (May 25) | Python upstream (vendor) |
|--------|-------------|--------------------------|--------------------------|
| R@5 | **43.4%** | 43.6% | 96.6% |
| R@10 | **50.4%** | 50.6% | — |
| MRR | **0.280** | 0.280 | — |
| Scored | 500 / 500 | 500 / 500 | 500 / 500 |
| Mean wall-time / question | **4,109 ms** | 4.3 ms | — |
| Total wall time (500 q) | **34 min** | ~2 s | — |

## What this measures

The retrieval path exercised is `mempalace_core::searcher::search_memories` which calls
`open_for_search` then `palace_db.query()`. The critical chain:

1. **`open_for_search`** (searcher.rs:18-37) attempts `PalaceDb::open_with_embedder` with
   `bge-small-en-v15`. This re-syncs all stored embeddings from scratch on every open
   (`sync_embeddings`), which is the dominant time cost (~4 s / question).
2. **`palace_db.query()`** (palace_db.rs:1785-1793) delegates to `query_sync`, which performs
   `naive_similarity` — a Jaccard-like keyword overlap with a 0.05 cutoff. **No vector
   similarity is used during the query.** The BM25 index and HNSW index are both present
   in memory but not consulted at query time.

The numbers confirm that the query logic is unchanged from Phase 0: recall is identical
within measurement noise (43.4% vs 43.6%; 217 vs 218 hits at R@5). The 1000x slowdown
(4.1 s vs 4.3 ms) is the cost of `sync_embeddings` at startup, which re-embeds every
drawer into the HNSW index but never queries it.

## Per-question-type breakdown

| Question type | N | R@5 | R@10 | MRR | Mean elapsed (ms) |
|---|---|---|---|---|---|
| single-session-user | 70 | **0.1429** | 0.2143 | 0.1034 | 5,678 |
| single-session-assistant | 56 | **0.7500** | 0.8036 | 0.6351 | 4,109 |
| single-session-preference | 30 | **0.3667** | 0.5333 | 0.1810 | 3,892 |
| multi-session | 133 | **0.3910** | 0.4887 | 0.2126 | 4,008 |
| knowledge-update | 78 | **0.6667** | 0.6795 | 0.4640 | 3,606 |
| temporal-reasoning | 133 | **0.3759** | 0.4361 | 0.2035 | 3,727 |
| **overall** | **500** | **0.434** | **0.504** | **0.280** | **4,109** |

These per-type numbers are within ± 0.005 of the Phase 0 lexical baseline. No question
type shows a statistically significant improvement or regression.

## Questions returning zero results

38 questions returned 0 results (same as Phase 0). These are cases where the `naive_similarity`
keyword overlap between the query and all haystack sessions falls below the 0.05 cutoff. In
many cases the answer is a session whose user-turn text has no lexical overlap with the
question.

## Why recall did not improve

The v0.3.0 release added `open_with_embedder` (vector embedder at startup, ADR-8), BM25
indexing (`bm25::SearchEngine`), the HNSW vector index (`EmbeddingDb`), and `hybrid_search`
with RRF fusion. However, `searcher::search_memories` still calls `palace_db.query()` which
resolves to `query_sync` — the same lexical Jaccard-like path measured in Phase 0.

The `hybrid_search` method (palace_db.rs:1887) that actually fuses BM25 + vector + graph
search is implemented but unreachable from the production search API.

## Timing breakdown

| Component | Mean per question |
|-----------|------------------|
| Mine (JSON parsing + insert) | 36 ms |
| Search (`naive_similarity` scan) | 38 ms |
| Open + sync_embeddings (embedded in search time) | ~4,035 ms |
| **Total** | **4,109 ms** |

The search call reported 4,073 ms mean; the open + sync_embeddings step happens inside
`open_for_search` which is called before `query`. The lexical scan itself is fast (~38 ms);
the vector re-embedding dominates.

## Comparison to Phase 0 (May 25 lexical baseline)

| Aspect | Phase 0 (May 25) | v0.3.0 (this run) | Delta |
|--------|-------------------|--------------------|--------|
| R@5 | 0.4360 | 0.4340 | −0.002 |
| R@10 | 0.5060 | 0.5040 | −0.002 |
| MRR | 0.2798 | 0.2795 | −0.0003 |
| q/0 results | 38 | 38 | 0 |
| Mean time | 4.3 ms | 4,109 ms | +4,105 ms |

The recall numbers are statistically indistinguishable. The only change is the ~4 s per
query overhead for embedding sync that this version pays but does not use.

## What would be needed for parity with the Python upstream (96.6%)

1. **Switch `search_memories` to call `palace_db.hybrid_search`** instead of `palace_db.query`.
   This requires selecting a fusion/rerank strategy (RRF default), and confirming the
   vector HNSW index is populated (it is, via `sync_embeddings` inside `open_with_embedder`).
2. **Fix per-question overhead.** The current `open_with_embedder` → `sync_embeddings`
   path re-embeds every drawer from scratch on every query. For a 53-drawer haystack this
   takes ~4 s. Caching or persisting embeddings between `PalaceDb` open/close cycles would
   bring this down. Alternatively, the benchmark harness could hold the palace open across
   queries (but this breaks per-question palace isolation, which is the design that makes
   R@k meaningful).
3. **Validate BM25 rebuild.** The current `open_collection` and `open_collection_with_embedder`
   construct a fresh `bm25::SearchEngine` from scratch (using `SearchEngineBuilder::with_avgdl`).
   The BM25 index does not persist between opens, so every `query` on a freshly opened palace
   has no BM25 index until `hybrid_search` builds it from the document map. If `hybrid_search`
   is called, the BM25 index used is the empty one — documents must be re-indexed first.
4. **Consider a better embedding model.** BGE-small at 384 dimensions may not be sufficient
   to distinguish "what city did I say I'd visit?" from "flying to Madrid for a conference."
   The Python upstream uses `all-MiniLM-L6-v2` (384d) via Chroma's ONNX path. The Rust port
   uses `bge-small-en-v15` (384d) via fastembed. Both are comparable in capacity; the gap
   relative to the Python upstream is likely the retrieval algorithm (pure vector + BM25
   rerank vs naive Jaccard), not the model choice.

## How to reproduce

```bash
# from repo root
cargo run --release -p mempalace-bench --bin longmemeval-bench \
    -- --output target/longmemeval_results.json \
       > target/longmemeval_results.ndjson
```

The results JSON used for this report is at:
`/Users/tranquangdang21/Projects/mempalace_rust/target/longmemeval_results.json`

## Cross-reference

| System | LongMemEval-S R@5 | Source |
|--------|------------------|--------|
| MemPalace (Python upstream, vendor) | 96.6% | mempalace README |
| mempalace (rohitg00) | 95.2% | report 06 section 1 |
| Mem0 (Nov 2025 algorithm, vendor) | 94.4% | report 02 section 1306 |
| Vectorize Hindsight | >= 90% | report 02 section 1306 |
| Long-context gpt-4o (Zep paper baseline) | 60.2% | report 02 section 1306 |
| **MemPalace (Rust v0.3.0, naive lexical Query)** | **43.4%** | **this doc** |
| MemPalace (Rust Phase 0, naive lexical) | 43.6% | docs/research/06_phase0_longmemeval_baseline.md |
