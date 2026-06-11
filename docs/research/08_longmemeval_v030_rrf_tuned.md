# LongMemEval-S Benchmark — mempalace_rust v0.3.0 (RRF Tuned + BM25 Re-ranker)

**Date:** 2026-06-11
**Harness:** `mp-003.v1`
**Embedding model:** `bge-small-en-v15` (fastembed)

## Changes from baseline (v0.3.0 vanilla)

1. **Fixed BM25 empty in embedder path** — `open_collection_with_embedder` now rebuilds BM25 index from documents after `sync_embeddings`, so `hybrid_search` has a populated BM25 stream alongside vector + graph.
2. **Fixed `return Ok(vec![])` bug** — embedding failure no longer aborts the entire search; vector stream simply returns empty and BM25 + graph continue.
3. **RRF_K: 60 → 25** — sharper ranking differentiation within each search stream.
4. **Enabled external BM25 re-ranker** — `search_memories` now uses `use_bm25=true`, re-ranking top 30 results with 70% hybrid similarity + 30% BM25 score.

## Results summary

| Metric | Before (Jaccard) | After fixes (v0.3.0) | **This run** | Python target |
|--------|:---:|:---:|:---:|:---:|
| **R@5** | **43.4%** | **82.4%** | **88.8%** 🔥 | **96.6%** |
| **R@10** | **50.4%** | **97.4%** | **93.4%** | — |
| **MRR** | **0.280** | **0.552** | **0.763** 🔥 | — |

### Per-type breakdown

| Type | n | R@5 | R@10 | MRR |
|---|---|---|---|---|
| knowledge-update | 78 | **98.7%** 🎉 | 100% | 0.916 |
| single-session-user | 70 | **92.9%** | 95.7% | 0.761 |
| single-session-assistant | 56 | **89.3%** | 89.3% | 0.740 |
| temporal-reasoning | 133 | **88.0%** | 91.7% | 0.746 |
| multi-session | 133 | **87.2%** | 94.0% | 0.770 |
| single-session-preference | 30 | **63.3%** | 83.3% | 0.467 |
| **TOTAL** | **500** | **88.8%** | **93.4%** | **0.763** |

### Improvement analysis

| Type | Before (no re-ranker) | After (with re-ranker) | Δ |
|---|---|---|---|
| knowledge-update | 89.7% | **98.7%** | +9.0pp ✅ |
| single-session-assistant | 60.7% | **89.3%** | +28.6pp ✅ |
| single-session-user | 81.4% | **92.9%** | +11.5pp ✅ |
| temporal-reasoning | 83.5% | **88.0%** | +4.5pp ✅ |
| multi-session | 91.7% | 87.2% | -4.5pp ⚠️ |
| single-session-preference | 60.0% | 63.3% | +3.3pp → |

The BM25 re-ranker boosts R@5 across most types but slightly hurts multi-session. R@10 dropped from 97.4% to 93.4% as the re-ranker pushes non-keyword-matching correct answers below position 10.

## Remaining gap to target (96.6% R@5)

To close the 7.8pp gap:
1. **Multi-vector per document** — sentence-level chunking for finer-grained search
2. **Persistent embeddings** — avoid recomputing on every `open` (3-6s penalty)
3. **Two-stage re-ranking** — cross-encoder on top of initial BM25+vector retrieval
4. **Better embedding model** — try `all-MiniLM-L6-v2`

## Timing

| Component | Mean per question |
|-----------|------------------|
| Mine | 38 ms |
| Open + sync_embeddings (dominant) | ~5,470 ms |
| Search (hybrid + BM25 re-rank) | 10 ms |
| **Total** | **5,510 ms** |

Slowdown vs Jaccard (~4.1 ms) is entirely `sync_embeddings` recomputing vectors from scratch on every open.

## Command

```bash
cargo run --bin longmemeval-bench
```
