# Phase 0 — LongMemEval-S baseline (Rust port)

> Status: **captured locally on 2026-05-25** with harness `mp-003.v1`.
> Issue: [`mr-mp-003-ktw` / mp-003] · plan §4 Phase 0 · report 02 §1306 · report 06 §1.

## TL;DR

The Rust port's *current* retrieval path — `searcher::search_memories`
backed by `palace_db::query_sync` (Jaccard-style "naive_similarity"
keyword overlap, **no vector embeddings**) — scores **R@5 = 43.6 %** on
the public LongMemEval-S split. The Python upstream's marketing number
of **96.6 %** is **not reproduced** by the Rust port.

This was the entire point of Phase 0: replace the inherited
README claim with a number we actually measured.

## Headline numbers

| Metric | Rust port (this run) | Python upstream (README) | Δ |
|---|---|---|---|
| Recall@5 | **0.4360** | 0.966 | −0.530 |
| Recall@10 | **0.5060** | — | — |
| MRR | **0.2798** | — | — |
| Scored questions | 500 / 500 | 500 / 500 | ✓ |
| Skipped | 0 | — | — |
| Mean wall-time per question | 4.3 ms | — | — |

> `MRR` is the mean of `1 / rank_of_first_correct_session`, 1-indexed,
> over the `limit=10` retrieved set. Questions where no correct session
> appears in the top-10 contribute 0.

## Per-question-type breakdown

| Question type | N | R@5 | R@10 | MRR |
|---|---|---|---|---|
| knowledge-update | 78 | 0.667 | 0.679 | 0.465 |
| multi-session | 133 | 0.391 | 0.489 | 0.212 |
| single-session-assistant | 56 | **0.750** | **0.804** | **0.635** |
| single-session-preference | 30 | 0.367 | 0.533 | 0.181 |
| single-session-user | 70 | 0.143 | 0.214 | 0.103 |
| temporal-reasoning | 133 | 0.383 | 0.444 | 0.204 |
| **overall** | **500** | **0.436** | **0.506** | **0.280** |

The high score on `single-session-assistant` (0.750 / 0.804 / 0.635)
makes sense for a keyword-overlap retriever: those questions paraphrase
the assistant's own prose, which gives the haystack-vs-query token
intersection a strong signal. The low score on `single-session-user`
(0.143 / 0.214 / 0.103) is the worst case for the same reason: the user
turn that contains the answer rarely lexically overlaps with the
question that asks about it ("what city did I mention I'd visit?" vs.
"flying to Madrid for a conference next week").

## Why these numbers and the Python 96.6 % differ

The Rust port's `palace_db::query_sync` (and therefore
`searcher::search_memories`) is **lexical**, not semantic:

```rust
// crates/core/src/palace_db.rs (paraphrased)
let similarity = naive_similarity(&query_lower, &entry.content.to_lowercase());
if similarity > 0.05 { /* keep */ }
```

`naive_similarity` is keyword overlap with a 0.05 cutoff. There is **no
embedding pass on the query side, no ANN index, no BM25**. The ONNX
embedder (`crates/core/src/onnx_embed.rs`) only runs at *write* time
inside the bench `EmbeddingDb` test path; nothing in the production
search path consumes the vectors it produces.

The Python upstream's 96.6 % uses a real ChromaDB HNSW index over
ONNXMiniLM_L6_V2 embeddings, plus optional BM25 rerank. That's the gap
this number measures, end to end. Closing it is exactly what plan §4
Phase 1 (`Native Embedder`) and Phase 5 (`Advanced Retrieval`) commit
to — Phase 0's job is to nail this baseline down so subsequent phases
can claim a defensible delta.

## How to reproduce

```bash
# from repo root
cargo run --release -p mempalace-bench --bin longmemeval-bench \
    -- --output target/longmemeval_results.json \
       > target/longmemeval_results.ndjson
```

First run downloads `longmemeval_s.json` (~277 MB) into
`crates/bench/data/longmemeval_s/` from the public HuggingFace mirror
`xiaowu0162/longmemeval-cleaned`. The dataset is **never committed** —
see `.gitignore`.

Useful flags:

| Flag | Effect |
|---|---|
| `--self-test` | Run a synthetic 1-question fixture with no network. |
| `--offline` | Skip download; error cleanly if the file is missing. |
| `--limit N` | Only evaluate the first N questions (smoke). |
| `--dataset PATH` | Override JSON location (skip cache logic entirely). |
| `--palace-root PATH` | Override per-question palace root (default: `target/longmemeval_palace`). |
| `--output PATH` | Override JSON report path (default: `target/longmemeval_results.json`). |

Outputs:

* **NDJSON** on stdout — one `{"type":"question",...}` per question,
  one `{"type":"summary",...}` footer. Diff-friendly.
* **JSON report** at `--output`. Pretty-printed, includes every
  per-question record plus the summary.

## Harness design notes (for the next agent)

* **Per-question fresh palace.** Each question's haystack is mined into
  `target/longmemeval_palace/<question_id>/`, wiped beforehand. This
  isolates the search index so R@k is over only the relevant haystack.
* **Drawer mapping.** Every haystack session becomes one drawer with
  `wing="longmemeval"`, `room="haystack"`, and `source_file=<session_id>`.
  `SearchResult.source_file` (which `From<QueryResult>` derives via
  `PathBuf::file_name`) round-trips the session_id unchanged because
  no path separators are present.
* **Scoring.** `recall_at_k` is "any hit in top-k" (matches Python
  reference `longmemeval_bench.py:71-74`). `mrr` is computed only over
  the `limit=10` returned slice, contributing 0 when no correct session
  appears.
* **Network failure mode.** Per the issue brief, `--offline` exits
  cleanly when the dataset is missing. If you're rerunning this in a
  sandbox without HF reachability, use `--dataset` to point at a
  pre-staged JSON.

## Baseline freeze

The harness stamps every NDJSON summary line with `harness_version =
"mp-003.v1"`. Bumping that constant in
`crates/bench/src/longmemeval_harness.rs::HARNESS_VERSION` invalidates
this baseline; Phase 1+ work that changes the corpus construction or
scoring code **must** bump it and refresh this document.

## Cross-reference: the field today

| System | LongMemEval-S R@5 | Source |
|---|---|---|
| MemPalace (Python upstream, vendor) | 0.966 | mempalace README |
| agentmemory (rohitg00) | 0.952 | report 06 §1 |
| Mem0 (Nov 2025 algorithm, vendor) | 0.944 | report 02 §1306 |
| Vectorize Hindsight | ≥ 0.90 | report 02 §1306 |
| **MemPalace (Rust port, naive_similarity)** | **0.436** | **this doc** |
| Long-context gpt-4o (Zep paper baseline) | 0.602 | report 02 §1306 |

This is the number `README.md` should quote until the Phase 1 native
embedder lands and we re-baseline.
