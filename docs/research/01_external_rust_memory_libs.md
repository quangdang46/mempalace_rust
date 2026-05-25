# External Rust Memory & Agent-Memory Ecosystem (2025-2026)

> Research brief for upgrading `mempalace_rust`. Snapshot date: **2026-05-25**.
> Current stack: `embedvec = 0.5` + `edgebert = 0.4` + `rusqlite` + `rmcp = 1.3`.
> Goal: identify libraries / frameworks / patterns to adopt, replace, or borrow from.

---

## TL;DR — TOP 5 must-evaluate shortlist

| # | Candidate | Why it matters for MemPalace | Adoption shape |
|---|-----------|------------------------------|----------------|
| 1 | **`fastembed-rs` (v5.x)** | Drop-in replacement for the Python ONNX subprocess. 30+ models incl. `BAAI/bge-m3`, `multilingual-e5-large`, `nomic-embed-text-v2-moe`, `Qwen3-Embedding`, `mxbai`, `snowflake-arctic`. Sync API, no tokio dep, ONNX via `ort`. Apache-2.0, very active (v5.13.4 — Apr 2026). | Replace `edgebert` + Python ONNX path. Add a `MEMPALACE_EMBED_MODEL` enum mapping to `fastembed::EmbeddingModel`. |
| 2 | **`tantivy` (BM25)** | First-class embedded BM25 + rich query language. Currently MemPalace does ad-hoc 70/30 vector+BM25 fusion in `searcher.rs`. Tantivy gives durable, persistent, segment-merged BM25 indexes that rival Lucene. | Add a tantivy index per palace, kept in sync with embedvec drawer IDs. Use as the BM25 leg of hybrid search → matches Mem0's semantic+BM25+entity fusion. |
| 3 | **`arroy` / `hannoy` (Meilisearch)** or **`usearch` (Unum)** | If `embedvec` becomes a bottleneck (no payload filtering, no quantization, single-flat index), arroy is LMDB-backed disk-resident ANN with filtered search; usearch is the fastest pure-HNSW (used by ClickHouse, DuckDB) with SIMD + JIT custom metrics. Both are embedded, no server. | Replace `embedvec` for the L2/L3 search layer; arroy if you want filterable disk ANN, usearch if you want raw speed + SQ8/PQ quantization. |
| 4 | **A-MEM (Zettelkasten) architecture** (NeurIPS 2025) | The palace already uses *wing→hall→room→closet→drawer*. A-MEM contributes the missing piece: **link generation + memory evolution**. Each new note's keywords/tags/context are LLM-generated, top-k linked, and old notes are *updated* in light of new ones. +24% over MemGPT on temporal LoCoMo. | Add a "tunnel evolution" pass: when a new drawer enters a room with siblings, re-evaluate sibling closets and refresh their AAAK summary. Keep this off by default (LLM cost), expose as `mpr evolve --wing X`. |
| 5 | **Mem0 token-efficient algorithm** (Apr 2026, 94.4 LongMemEval) | Mem0's 2026 architecture beats every published system at ~6.9k tokens/query: single-pass ADD-only extraction + **multi-signal retrieval** (semantic ⊕ BM25 ⊕ entity matching, fused). Stores agent confirmations as first-class facts. | Adopt the *fusion scoring formula* (semantic + BM25 + entity-graph hits) in `searcher.rs`. Treat AI-generated decisions in conversations as first-class drawers, not just user utterances. |

Reasoning: **(1)** removes the Python dependency that complicates packaging; **(2)** gives durable BM25 instead of in-memory toy; **(3)** is the contingency plan if embedvec hits walls; **(4)** is the architectural step that turns MemPalace from a passive store into an actively managed knowledge network; **(5)** is the single biggest LongMemEval-score lever currently on the table.

---

## 1. Rust-native memory / vector libraries

### 1.1 Comparison table

| Crate | License | Last release | Index type(s) | Persistence | Payload filtering | Async | Embedded? | Notes |
|-------|---------|--------------|--------------|-------------|-------------------|-------|-----------|-------|
| **`embedvec` 0.5** | (current) | 2025 | flat / brute force | file-backed | metadata-only filter | sync | ✅ in-process | What MemPalace uses now. No HNSW. |
| **`qdrant` (server)** | Apache-2.0 | very active | HNSW, sparse, multi-vector | RocksDB-style | full payload conditions | gRPC/REST | ❌ requires server | Industry leader, but server-only OSS path. |
| **Qdrant Edge** | Closed beta (2025-07) | Beta | HNSW | embedded | yes | sync | ✅ in-process | Currently private beta — not adoptable today. |
| **`lancedb`** | Apache-2.0 | 2026 active | IVF-PQ, HNSW (newer), flat | Lance columnar (Arrow) | full SQL-like | async | ✅ in-process | Embeds inside your binary; reads/writes Lance files; supports object storage. Heavy DataFusion dep. |
| **`surrealdb` (kv-mem / embedded)** | Apache-2.0 | very active | in-mem HNSW (`<\|K,EF\|>`) | RocksDB or memory | yes (SurrealQL) | async | ✅ embeddable as crate | Full graph+vector+FTS in one. Big dep tree. |
| **`hannoy`** (Meilisearch) | MIT | 2025-12 | HNSW on KV | LMDB | filtered search | sync | ✅ | Used inside Meilisearch. KV-backed HNSW. |
| **`arroy`** (Meilisearch) | MIT | 2025 | random-projection trees (Annoy-style) | LMDB | filtered disk ANN | sync | ✅ | Disk-resident, multi-process safe. Slower than HNSW for high dims, fast for filtered. |
| **`usearch`** (Unum) | Apache-2.0 | very active | HNSW + PQ + SQ + JIT custom metrics | mmap file | basic | sync | ✅ | Fastest single-file HNSW; user-defined metrics; SIMD. C++ core, Rust binding. |
| **`hnsw_rs`** (Jean-Pierre Both) | MIT | 2025 | HNSW (pure Rust) | bincode | none | sync | ✅ | Solid pure-Rust HNSW; no payload, you bring your own metadata. |
| **`instant-distance`** (djc) | Apache-2.0 / MIT | 2025-11 | HNSW (pure Rust) | bincode | none | sync | ✅ | ~2-13MB binary delta, very small footprint. |
| **`hnsw`** | Apache-2.0 / MIT | older | generic HNSW | none | none | sync | ✅ | Type-generic, low-level. |
| **`oasysdb`** | Apache-2.0 | 2025 | HNSW | sled / file | tag filter | sync | ✅ | "SQLite for vectors" niche; small but present. |
| **`tantivy`** | MIT | very active | inverted (BM25, phrase) | segment-merged | full query DSL | sync (rayon) | ✅ | Lucene-grade BM25, embedded. **Pair this with HNSW for hybrid.** |
| **`bm25` (Michael-JB)** | MIT | 2025 | in-mem BM25 scoring | none | n/a | sync | ✅ | Tiny scorer crate; no persistence. |
| **`redb`** | MIT/Apache | very active | KV (B-tree, COW) | mmap | n/a | sync | ✅ | ACID embedded KV (LMDB-style, pure Rust). Use for KG triples, episodic store. |
| **`fjall`** | MIT | 3.0 (2025) | KV (LSM) | log-structured | n/a | sync | ✅ | RocksDB in Rust. v3 outperforms sled/redb on most workloads per maintainer benches. |
| **`sled`** | MIT/Apache | mostly idle | KV (B-Ɛ tree) | log-structured | n/a | sync | ✅ | Stalled; community migrating to redb/fjall. |
| **`HelixDB`** | MIT | 2025 | graph + vector | RocksDB | yes | async | ✅ | Native graph-vector store in Rust. Worth tracking. |
| **`vectordb`** | varied | mixed | (umbrella name) | — | — | — | — | Mostly a name reused across crates; no single canonical project. |

### 1.2 What MemPalace could borrow

- **From tantivy:** segmented indexing model (merge small segments into large ones) maps perfectly onto closet/drawer churn.
- **From arroy:** LMDB-backed multi-process safety so multiple `mpr` processes can read the palace concurrently without lock contention (the current PID-file guard is a workaround).
- **From usearch:** SQ8/PQ quantization. Memory budget for a 96.6%-recall palace would drop ~4× with SQ8 and ~32× with PQ, basically free at this recall floor.
- **From fjall:** if `embedvec` is replaced piecewise (vectors stay in HNSW lib, payloads + KG move to fjall/redb), a single LSM tree keeps the operational story simple.
- **Skip surrealdb / lancedb:** both pull in massive dep trees (DataFusion / RocksDB / Tokio). Wrong tradeoff for a single-binary CLI tool.

### 1.3 Filtering / payload story

Most pure-HNSW crates (hnsw_rs, instant-distance, usearch) **don't do payload filtering** — you have to pre-filter or post-filter externally. MemPalace already has wing/room as first-class structure, and 99% of queries are scoped (`--wing X --room Y`). Implication: the ANN library only needs **per-room sub-indexes** (one HNSW per (wing, room)) plus a global index for cross-wing search. That keeps each index small and obviates payload filtering at the ANN layer.

---

## 2. Agent-memory frameworks (Rust or Rust-compatible)

### 2.1 Rust-native frameworks

| Project | License | Stars | Last release | What it offers | Memory model |
|---------|---------|-------|--------------|----------------|--------------|
| **`rig` / `rig-core`** (0xPlaygrounds) | MIT | 7.4k | v0.37 (2026-05) | LLM provider abstraction (20+), embedding workflows, vector store integrations, agent loops | `rig::memory` module — conversation history traits + in-memory backend; reusable history-shaping policies; integrations with LanceDB, Qdrant, SQLite, Postgres, Mongo, Neo4j, Surreal, Milvus, Scylla, S3Vectors, FastEmbed |
| **`swiftide` (bosun-ai)** | MIT | 700+ | v0.32 (2025-11) | Streaming indexing pipelines, query pipelines, agents with tools, Langfuse | Storage backends: Qdrant, Redis, LanceDB, Postgres, DuckDB; node-cache (Redis) for incremental indexing; no first-class long-term memory module — you wire one yourself |
| **`langchain-rust` (Abraxas-365)** | MIT | medium | 2025 active | LangChain-style chains, agents, memory, RAG, BM25, hybrid retrieval, LangGraph | Buffer/window memory, vector-backed memory; thin wrappers, useful for prototyping |
| **`langchainrust` (separate crate)** | (varies) | small | 2026 | LangGraph + HyDE + reranking + multi-query | Active recent fork |
| **`llmchain-rs`** | (older project) | — | mostly idle | Early Rust LLM chain | Not recommended for new work |
| **`memvid` (Rust impl)** | MIT | 1k+ | 2025 active | "Single-file memory layer" — packages embeddings + metadata + BM25 in one portable file; offline-capable | Hybrid (BM25 + vector) + entity extraction + time-travel debugging in a single file. Closest spiritual cousin to MemPalace's drawers. |
| **CocoIndex** | Apache-2.0 | active | v1 (2026-04) | Rust-core ETL framework for AI: incremental processing, change detection, lineage, schema evolution | Not memory itself; **the right ingestion layer** for keeping a palace continuously fresh. Rust core, Python bindings. |
| **Cortex Memory** (sopaco) | (Open) | small | 2026 | "production-ready memory system for intelligent agents" REST + MCP + CLI + dashboard, built on `rig` | Worth a deep code-read; targets the same niche as MemPalace. |
| **`synaptic-memory`** | varies | small | 2025 | LangChain-compatible Rust agent framework | Unknown maturity. |

### 2.2 Python frameworks (architectural lessons, not direct use)

| Project | License | Architecture | What MemPalace could copy |
|---------|---------|--------------|---------------------------|
| **Mem0** (`mem0ai/mem0`) | Apache-2.0 | Single-pass ADD-only extraction → vector store + optional graph; multi-signal retrieval (semantic+BM25+entity). Cloud + self-host + OpenMemory MCP variants. | Fusion-score formula, ADD-only memory write semantics, async-by-default writes, structured exceptions, `user_id`/`agent_id`/`run_id`/`app_id` scoping. **94.4 LongMemEval, 92.5 LoCoMo at 6.9k tokens/query.** |
| **Letta / MemGPT** (`letta-ai/letta`) | Apache-2.0 | OS-style memory tiers (core/working/recall/archival), "pages" between tiers, **sleep-time agents** that share blocks and run in background | Sleep-time / background "consolidation" agent — a periodic process that walks halls/rooms and refreshes critical-facts L1. Multi-agent shared memory blocks. |
| **Zep + Graphiti** | Apache-2.0 (Graphiti) | Temporal knowledge graph with bi-temporal validity (`valid_at` / `invalid_at`), incremental ingestion, Neo4j/FalkorDB backend | MemPalace already has temporal triples in SQLite. Borrow Graphiti's **conflict resolution rules** and **fusion of time + full-text + semantic + graph search**. Skip the Neo4j dep. |
| **A-MEM** (NeurIPS 2025) | (research) | LLM-generated keywords/tags/context per note → embedding over concatenated fields → top-k cosine + LLM filter for link generation → **memory evolution** rewrites old notes | Three-step pipeline: enrich, link, evolve. **+18 F1 over MemGPT temporal**, sub-microsecond retrieval at 1M entries. See §5.1. |
| **Memori** (GibsonAI) | LLM-agnostic | Persistent memory layer over standard SQL DBs (Postgres, SQLite, CockroachDB) — memory as a *data structuring problem*. Triples + summaries. | Validates MemPalace's "use boring SQL" choice. Their schema for triple+summary+session is worth a side-by-side compare. |
| **Vectorize Hindsight** (open-source, Dec 2025) | MIT | First open-source system to clear 90% on LongMemEval | Architecture not yet fully public — track the repo. |
| **Mastra Observational Memory** | proprietary | 95% on LongMemEval (gpt-4o), beats oracle | Closed source; idea: agent observes its own retrievals and learns from outcomes. Same direction as MemPalace's "episodic helpfulness scores." |
| **LangMem** (LangChain) | MIT | Tuple-namespace memories `(user, app, "context")` with metadata + semantic search | Validates MemPalace's wing/room/hall structure. |

### 2.3 Architectural pattern catalog (what to copy, in priority order)

1. **Multi-signal retrieval fusion** (Mem0 2026): score = w1·semantic + w2·BM25 + w3·entity-match. MemPalace's current 70/30 vector+BM25 weight blend → upgrade to 3-signal fusion using the knowledge-graph entities you already extract.
2. **First-class agent-confirmation memories** (Mem0 ADD-only): when the AI says "I see you decided X," that becomes a drawer too, tagged with `provenance=agent_inferred`. Currently MemPalace only mines user utterances.
3. **A-MEM evolution loop** (NeurIPS 2025): periodic pass that re-summarizes closets when new sibling drawers arrive. Throttle by AAAK-token-budget, not document count.
4. **Sleep-time consolidation** (Letta): a `mpr consolidate` daemon that runs during idle time, regenerates L1, updates KG triples, and prunes stale episodic scores. Map this to existing `mpr wake-up` and `mpr doctor`.
5. **Bi-temporal validity** (Graphiti): MemPalace already has `valid_from` / `ended` — extend to two timestamps: `transaction_time` (when we learned it) vs `valid_time` (when it actually started). Critical for "what did we believe in March?" queries.
6. **Multi-scope identity** (Mem0): `user_id` + `agent_id` + `run_id` + `app_id` as compositional retrieval keys. MemPalace's wings are user/project; add `agent_id` for the diary system.
7. **Procedural memory tier** (Mem0 open problem call-out): besides episodic ("what happened") and semantic ("what is true"), store *how things should be done*. Map to MemPalace's `hall_advice`.

---

## 3. Embedding inference in Rust (replacing the Python ONNX subprocess)

### 3.1 Comparison table

| Library | License | Status | Backend | Models supported | Key facts |
|---------|---------|--------|---------|------------------|-----------|
| **`fastembed-rs`** (Anush008) | Apache-2.0 | v5.13.4 (Apr 2026), 901★, very active | `ort` (ONNX Runtime) + `candle` (for some) | `BAAI/bge-m3`, `bge-{small,base,large}-{en,zh}-v1.5`, `all-MiniLM-{L6,L12}-v2`, `all-mpnet-base-v2`, `paraphrase-multilingual-mpnet`, `nomic-embed-text-v1/v1.5/v2-moe`, `multilingual-e5-{small,base,large}`, `mxbai-embed-large-v1`, `gte-{base,large}-en-v1.5`, `ModernBERT-embed-large`, `jina-embeddings-v2-{en,code}`, `embeddinggemma-300m`, `Qwen3-Embedding-{0.6B,4B,8B}`, `Qwen3-VL-Embedding-2B`, `snowflake-arctic-embed-{xs,s,m,m-long,l}`. SPLADE sparse. CLIP image. BGE/Jina rerankers. Quantized variants. DirectML on Windows. | **The default choice.** Sync (no tokio dep), HF tokenizers, automatic model download. |
| **`ort` (pykeio)** | Apache-2.0 | very active | wraps ONNX Runtime | any ONNX model | The runtime everyone wraps. Use directly only if you need custom EPs (CUDA, TensorRT, OpenVINO, CoreML, DirectML). |
| **`candle` (HuggingFace)** | Apache-2.0 | very active | pure-Rust + CUDA/Metal | BERT, BGE, Jina, Nomic, Qwen3, Llama, Whisper, etc. via separate crates | Pure Rust ML; ~47% faster BERT inference vs PyTorch in benches (markaicode 2025). Heavier compile time, larger binary. |
| **`burn`** | Apache-2.0 / MIT | very active | pluggable (NDArray, WGPU, Candle, LibTorch) | model zoo growing | Research-grade; production usage smaller than candle. |
| **`rust-bert`** | Apache-2.0 | older | tch-rs (libtorch) | BERT/DistilBERT/RoBERTa | Requires libtorch (~300MB). Avoid for single-binary distribution. |
| **`model2vec-rs`** (MinishLab) | MIT | 2026 | pure Rust | static embeddings distilled from BGE/MiniLM/etc. | **100-500× faster than sentence-transformers** at small accuracy cost. Multilingual `potion-multilingual-128M` distilled from `bge-m3`. **Best fit for L1 / wake-up / per-token streaming.** |
| **`embed-anything`** (StarlightSearch) | Apache-2.0 | 2026 | candle + ort | text+image+audio+video, multimodal | Minimalist Rust pipeline; Python bindings; growing fast. |
| **`mistral.rs`** (EricLBuehler) | MIT | very active | candle | LLMs + some embeddings | Primarily LLM inference; OpenAI-compatible server. Overkill for embeddings. |
| **`llama-cpp-2` / `llama_cpp`** | MIT | very active | llama.cpp bindings | GGUF embeddings (`bge-m3`, `nomic-embed-text-v2`, `jina-v5`) | Smallest disk-binary path; GGUF Q4_K is ~50MB for an embedding model. Worth considering for offline edge. |
| **`pylate-rs`** (LightOn) | Apache-2.0 | 2026 | candle | ColBERT | "97% faster ColBERT" — useful if you want late-interaction reranking. |
| **`edgebert`** (current) | (whatever 0.4 is) | seems older | ONNX | a few BERTs | What MemPalace uses today. Migration to fastembed-rs is straightforward. |

### 3.2 Recommendation

**Replace the Python ONNX subprocess + `edgebert` with `fastembed-rs`.**

- Delete `onnx_embed_python.py` and the subprocess plumbing.
- Map `MEMPALACE_EMBED_MODEL` env to `fastembed::EmbeddingModel` enum.
- Default → `EmbeddingModel::AllMiniLML6V2` (drop-in for current `ONNXMiniLM_L6_V2`, same 384 dims).
- Multilingual upgrade path → `MultilingualE5Base` (768) or `BGEM3` (1024).
- Wake-up / L1 generation → consider `model2vec-rs` with `potion-multilingual-128M` for sub-millisecond static embeddings.

**Net result:** single binary, no Python dependency, install.sh shrinks substantially, faster startup (<10ms vs 300ms Python boot).

### 3.3 Benchmarks (when findable)

- **Candle vs PyTorch (markaicode 2025-08):** BERT inference 47% faster, ResNet-50 35% faster, LLaMA-2 token-gen 38% quicker on Candle.
- **Mem0 token efficiency (Apr 2026):** 6,956 avg tokens/query on LoCoMo vs ~26,000 for full-context.
- **A-MEM (NeurIPS 2025):** ~1,200 tokens/op, 5.4s on GPT-4o-mini, 3.7µs retrieval at 1M entries.
- **fastembed-rs vs Python fastembed:** roughly equivalent throughput (same `ort` underneath); avoids Python serialization overhead → ~10-20% wall-clock improvement on small inputs.
- **model2vec-rs:** "100-500× faster than sentence-transformers" per Minish docs; trade-off is small MTEB drop (~5-10 points on retrieval tasks).

---

## 4. MCP servers for memory

### 4.1 Comparison table

| Server | Author | Stack | Data model | Tools (sample) |
|--------|--------|-------|------------|----------------|
| **`@modelcontextprotocol/server-memory`** (official) | Anthropic / MCP | TypeScript | **Knowledge graph**: Entities (name, type, observations) + Relations (active voice) | `create_entities`, `create_relations`, `add_observations`, `delete_*`, `read_graph`, `search_nodes`, `open_nodes` |
| **Graphiti MCP** (`getzep/graphiti`) | Zep | Python + Neo4j/FalkorDB + LLM | **Bi-temporal knowledge graph** with episodes, entities, relations, communities | `add_episode`, `search_facts`, `search_nodes`, `get_episode`, `delete_*`, hybrid time+text+semantic+graph search |
| **Mem0 MCP / OpenMemory MCP** | Mem0 | Python + (any vector store) | Memories scoped by user_id/agent_id/run_id/app_id | `add_memory`, `search_memory`, `update_memory`, `delete_memory`, async by default |
| **Basic Memory** (`basicmachines-co/basic-memory`) | Basic Machines | Python + plain Markdown files | **Markdown files** with frontmatter, observations, relations; Obsidian-compatible vault | `write_note`, `read_note`, `build_context`, `search_notes`, `recent_activity`, `list_directory`, `canvas` |
| **Knowledge Graph Memory Server** (yodakeisuke + others) | community | varied | Same shape as official + domain prompts | Same canonical entity/relation/observation tools |
| **`mcp-memory-server` (s2005)** | community | Standalone with persistence | Knowledge graph over local file | Same shape as official |
| **Memvid / claude-brain MCP** | Olow304 / memvid | Python + single-file capsule | Hybrid (BM25 + vector) + entities in one portable file | `search`, `recall`, `add`, `time_travel`, capsule export/import |
| **MemX (research, Mar 2026)** | Lizheng Sun | **Rust + libSQL** + OpenAI-compatible embeddings | Conversational memory with stability-oriented retrieval | Per arxiv 2603.16171 — stratified granularity, threshold rejection, latency analysis. Closest published Rust lineage to MemPalace. |
| **`graphiti-memory` (PyPI)** | community | Python + Neo4j + Graphiti | Same as Graphiti core | KG operations |
| **gemini-graphiti-mcp** (criticalinsight) | community | Graphiti + Mem0 + FalkorDB | Hybrid KG + token-efficient mem | Combined surface area |
| **MemPalace `mpr mcp`** (current) | this project | Rust + embedvec + SQLite | Wing/Hall/Room/Closet/Drawer + KG triples + agent diary | 19 tools (palace read/write, KG, diary) |

### 4.2 Common pattern: the canonical "knowledge graph" tool surface

The official Anthropic MCP memory server crystallized a 9-tool API that every clone re-implements:

```
create_entities      add_observations      delete_entities
create_relations     delete_observations   delete_relations
read_graph           search_nodes          open_nodes
```

Data shape:
```json
{
  "entities": [{"name": "John_Smith", "entityType": "person",
                "observations": ["Speaks fluent Spanish"]}],
  "relations": [{"from": "John_Smith", "to": "Acme",
                 "relationType": "works_at"}]
}
```

**Implication for MemPalace:** the existing `mpr_kg_*` tools already cover this surface. Naming alignment (`mpr_kg_add` ≈ `create_entities`+`create_relations`) would help cross-tool interoperability. Adding aliased tool names (`create_entities` → `mpr_kg_add` of type entity) would let any MCP client tuned for the official server use MemPalace as a drop-in upgrade.

### 4.3 Anthropic Memory Tool (Sept 2025, beta)

Distinct from the *server-memory* MCP server. This is a **client-side filesystem tool** built into the Claude API:

- Tool type: `memory_20250818`
- Operations: `view`, `create`, `str_replace`, `insert`, `delete`, `rename`
- All ops scoped to `/memories/` directory; client-side handler executes
- Anthropic ships ZDR (Zero Data Retention) compatibility
- Pairs with **context editing** + **compaction** for long-running sessions
- Dec 2025 → Anthropic added managed-agents server-side hosted memory; May 2026 → "dreaming" research preview for self-improving agents

**Implication for MemPalace:** expose the palace as an Anthropic Memory Tool backend. Map `view /memories/X` → MemPalace search. Map `create /memories/<wing>/<room>/<slug>.md` → `mpr_add_drawer`. This unifies MemPalace with the Claude-native memory workflow without users having to manage MCP config.

### 4.4 Basic Memory's data model is uncannily similar to MemPalace

- Markdown files = MemPalace drawers
- Frontmatter = MemPalace metadata
- `[[wikilinks]]` = MemPalace tunnels
- Tags = MemPalace halls
- Obsidian visualization = third-party graph view

If MemPalace adds a `mpr export --format basic-memory` (writes to a Markdown vault Obsidian can render) or `mpr import basic-memory <vault>`, you get free integration with that ecosystem.

---

## 5. Recent (2025-2026) papers and releases on agent memory

### 5.1 A-MEM: Agentic Memory for LLM Agents (NeurIPS 2025)

- **Paper:** arXiv:2502.12110, accepted NeurIPS 2025
- **Code:** github.com/agiresearch/A-MEM, github.com/WujiangXu/A-mem
- **Architecture:** Zettelkasten — every memory is a richly annotated note (content, timestamp, LLM-generated keywords, tags, contextual description, embedding, links). Three operations:
  1. **Note construction** — embed concatenation of all generated fields
  2. **Link generation** — top-k cosine + LLM filter to reject surface-level matches
  3. **Memory evolution** — when a new note links to old ones, the LLM rewrites the old notes
- **Results vs MemGPT (LoCoMo, F1):** GPT-4o-mini multi-hop 25.0→27.0; temporal 18.4→**45.85**; open-domain 12.0 baseline → 27+ in newer models. Ablation: link generation alone +12 F1, +memory evolution +6 more. ~1,200 tokens/op, 3.7µs retrieval at 1M entries.
- **Risks (per peer review):** error propagation through evolution loop; LLM-judgment-quality dependency; Zettelkasten "rot" if upkeep is poor.
- **What MemPalace borrows:** the *evolution* operation is the missing piece. MemPalace already has steps 1 and 2 (it stores rich closets and computes tunnels). Step 3 — when a new drawer enters a room, refresh sibling closet AAAK summaries — would close the loop.

### 5.2 Mem0: Building Production-Ready AI Agents with Scalable Long-Term Memory (ECAI 2025)

- **Paper:** arXiv:2504.19413
- **2026 update:** "Token-Efficient Memory Algorithm" (Apr 2026)
- **Numbers:** **94.4 LongMemEval, 92.5 LoCoMo, 64.1 BEAM-1M, 48.6 BEAM-10M** at ~6.9k tokens/query. +29.6 on temporal vs 2025 algorithm, +23.1 on multi-hop.
- **Architectural changes that drove the gain:**
  1. *Single-pass ADD-only extraction* — agent confirmations stored as first-class facts
  2. *Multi-signal retrieval* — semantic + BM25 + entity matching, fused
- **Production features that shipped:** async writes (default), reranking (Cohere/HF/Sentence-Transformers/LLM), metadata filtering, timestamp-on-update, memory-depth config, structured exceptions
- **What MemPalace borrows:** the fusion-score formula and the ADD-only semantics for agent inferences. See §2.3 #1 and #2.

### 5.3 MemGPT / Letta — Sleep-time compute

- **Paper:** arXiv:2504.13171 (Apr 2025) — "sleep-time compute"
- **Idea:** between user turns, run an agent in the background that reorganizes memory — pre-summarizes likely contexts, refreshes critical facts, anticipates queries. Up to **5× lower test-time tokens** for the same accuracy.
- **Letta product:** sleep-time agents share blocks with primary agents; they run async and can mutate memory.
- **What MemPalace borrows:** a `mpr consolidate --background` daemon mode. While the user is idle, walk recently-modified rooms and refresh closet AAAK + L1 critical facts. Map sleep-time block-sharing to MemPalace's wing-shared agent diaries.

### 5.4 LongMemEval & LoCoMo benchmark leaderboard (as of May 2026)

| System | LongMemEval (best) | LoCoMo (best) | API calls | Notes |
|--------|-------------------|---------------|-----------|-------|
| **Mem0 2026 algo** | **94.4** | **92.5** | yes | Token-efficient + multi-signal (Apr 2026) |
| Mastra Observational Memory | 95.0 (gpt-4o internal) | — | yes | "Beats oracle" on LME; closed source |
| Vectorize Hindsight | >90 | — | yes | Open source as of Dec 2025 |
| **MemPalace (Python ref, hybrid+Haiku)** | **100% (500/500)** | — | ~500 | Caveat: hybrid mode uses LLM rerank |
| **MemPalace (Python ref, raw)** | **96.6%** | — | 0 | Vector-only, no LLM |
| Supermemory ASMR | ~99 | — | yes | Closed |
| Mastra (DSPy) | 94.87 | — | yes | GPT-based |
| Mem0 (2025 paper) | 85 | — | yes | Original arxiv numbers |
| Zep | ~85 | — | yes | KG-based |
| MemGPT | — | 26.7 (multi-hop, GPT-4o-mini) | — | Earlier baseline |
| A-MEM | — | 27.0 multi-hop / **45.9 temporal** | — | NeurIPS 2025 |
| LoCoMo baseline (full-context) | — | 25 multi-hop / 18.4 temporal | — | Reference |

**Important:** MemPalace's headline 96.6% / 100% scores are from the *Python reference* implementation. The Rust port currently aims to *match*. To stay competitive in 2026, the Rust port needs the §2.3 upgrades (especially fusion scoring + A-MEM evolution).

### 5.5 Anthropic memory tool design (Sept 2025 → May 2026)

- **Sept 2025:** beta launch — file-directory based, client-side handler
- **Nov 2025:** advanced tool use APIs
- **Apr 2026:** managed-agents server-side memory in public beta
- **May 2026:** "dreaming" research preview (self-improving consolidation)
- **Design philosophy:** *memory = structured filesystem*, intentionally simple. The model calls tools; you store however you like. Pairs with context editing + compaction.

### 5.6 OpenAI memory architecture

No official paper. Public knowledge:
- ChatGPT consumer memory uses two layers: per-conversation context + cross-conversation "memory" (extracted facts)
- Extraction is LLM-driven, deduped against existing memories before storage
- User can view/edit/delete memories via UI
- API path: stateful via `responses.create({store: true, conversation_id})` since 2025

No leaked architectural details beyond this. Anything more specific would be speculation.

### 5.7 Generative Agents memory streams (Park et al., 2023)

- **Paper:** arXiv:2304.03442
- **Architecture:** flat memory stream (timestamped natural-language observations) + 3-component retrieval score:
  - **Recency** — exponential decay
  - **Importance** — LLM-rated (1-10) at write time
  - **Relevance** — cosine similarity to query
- **Reflection mechanism:** when sum of recent importance crosses a threshold, agent generates higher-level reflective memories from low-level ones (early A-MEM precursor).
- **What MemPalace already borrows:** episodic helpfulness scores (`+1`/`-1` on retrieval feedback) ≈ importance × recency. Adding an explicit recency-decay weight to search would close the gap.

### 5.8 Other notable 2025-2026 work

| Paper / Project | Date | Why it matters |
|-----------------|------|----------------|
| **Zep / Graphiti temporal KG** (arXiv:2501.13956) | Jan 2025 | Bi-temporal validity, beats MemGPT on DMR |
| **Memori paper** (arXiv:2603.19935) | 2026 | Memory-as-data-structuring, semantic triples + summaries on standard SQL |
| **"Cost-Performance Analysis of Fact-Based Memory vs. Long-Context"** (arXiv:2603.04814) | 2026 | Fact-based memory wins on cost-accuracy frontier across LongMemEval/LoCoMo/PersonaMem |
| **"Human-Inspired Memory Architecture"** (arXiv:2605.08538) | 2026 | Biologically-grounded: sleep-phase consolidation, interference-based forgetting, engram maturation, reconsolidation, hybrid retrieval. Validates A-MEM-style evolution from a different angle. |
| **MemX** (arXiv:2603.16171) | Mar 2026 | Local-first long-term memory **in Rust** on libSQL — direct competitor / cousin to MemPalace |
| **"Evaluating Long-Term Agent Memory Toward Experienced Colleagues"** (arXiv:2605.12493) | 2026 | New benchmark "LongMemEval-V2" with AgentRunbook; emphasis on procedural memory |
| **BEAM benchmark** (1M / 10M tokens) | 2025-2026 | New scale benchmark; cannot be solved by context expansion alone |
| **Memoria** (arXiv:2512.12686) | Dec 2025 | Dynamic session-level summarization + KG-based user model |
| **Vectorize Hindsight** | Dec 2025 | First open-source >90% LongMemEval |
| **Anthropic "dreaming"** | May 2026 | Research preview; periodic memory pattern detection |

---

## 6. Concrete recommendations for `mempalace_rust`

### 6.1 Near-term (low risk, high value)

| Action | Effort | Payoff |
|--------|--------|--------|
| Replace Python ONNX subprocess + `edgebert` with **`fastembed-rs`** | M | Single-binary install, no Python dep, supports BGE-M3 / multilingual-E5 / Qwen3-Embedding out of the box |
| Add **`tantivy`** for the BM25 leg of hybrid search | M | Persistent, high-quality BM25 instead of in-memory hack. Pairs with embedvec via shared drawer-id keys |
| Implement Mem0's **3-signal retrieval fusion** (semantic + BM25 + entity) | S | Direct LongMemEval boost; uses existing entity registry |
| Add **agent-confirmation ADD-only writes** (provenance=`agent_inferred`) | S | Captures decisions made *during* the AI session, not just user statements |
| Add **recency decay** + **importance** scores to existing helpfulness score (Generative Agents) | S | Closes the gap with the canonical retrieval triplet |
| Alias MCP tool names to match the official `@modelcontextprotocol/server-memory` surface (`create_entities`, etc.) | XS | Free interoperability with any tool tuned for the official server |
| Add **`mpr export --format basic-memory`** | S | Lets users render the palace in Obsidian for free |

### 6.2 Mid-term (architectural)

| Action | Effort | Payoff |
|--------|--------|--------|
| Implement **A-MEM evolution loop** (`mpr evolve`) | L | NeurIPS-grade architecture upgrade; +F1 on temporal/multi-hop |
| Implement **sleep-time consolidation** (`mpr consolidate --daemon`) | M | Letta-style background memory hygiene, generates fresh L1, prunes stale episodes |
| Add **bi-temporal validity** (`transaction_time` vs `valid_time`) to the KG | M | Graphiti parity; enables "what did we believe in March?" queries |
| **Sub-index per (wing, room)** in HNSW → eliminates need for payload filtering | M | Smaller indexes, faster recall, simpler code |
| Adopt **`fjall` or `redb`** for the KG triple store + episodic store, leaving vectors in embedvec/usearch/arroy | M | Pure-Rust, no SQLite dep for the high-write path |

### 6.3 Long-term / contingent

| Action | Trigger | Action |
|--------|---------|--------|
| Migrate ANN backend from `embedvec` to **`usearch`** (PQ/SQ8 quantization) | When palace size > 1M drawers OR memory budget bites | Drop-in HNSW with quantization; ~4-32× memory reduction |
| Migrate ANN to **`arroy`/`hannoy`** (LMDB-backed) | When you want multi-process safe concurrent reads | Get rid of the PID-file guard, allow concurrent `mpr search` |
| Adopt **`candle`** + **`model2vec-rs`** for L1 generation | When wake-up latency matters (<100ms target) | Static embeddings 100-500× faster; multilingual via `potion-multilingual-128M` |
| Add **CocoIndex** as the ingestion layer (incremental, change-detected) | When users want continuous palace freshness | Rust-core ETL, lineage-tracked, integrates with embedvec |
| Adopt **Anthropic Memory Tool backend** mode | When Claude's managed-agents matter to users | `mpr memory-tool` exposes a `/memories` directory backed by the palace |

---

## 7. Per-candidate one-liners (research index)

| Name | Verdict |
|------|---------|
| qdrant | Keep watching Qdrant Edge; today's open-source path is server-only. |
| lancedb | Powerful but heavyweight; wrong fit for single-binary CLI. |
| surrealdb-vector | All-in-one, but 50+ transitive deps. Pass. |
| **embedvec** | Currently in-tree. Sufficient until 1M+ drawers; then upgrade to usearch/arroy. |
| **usearch** | Fastest pure HNSW with quantization; #3 shortlist. |
| hnsw_rs | Solid pure-Rust HNSW; no payload filtering. |
| instant-distance | Tiny, pure Rust, well-maintained. Good fallback. |
| oasysdb | Niche; small project; not a default pick. |
| **arroy / hannoy** | LMDB-backed disk ANN with filtered search; multi-process safe. #3 shortlist alternative. |
| **tantivy** | THE Rust BM25. #2 shortlist. |
| redb | Use if you want pure-Rust ACID KV; replace SQLite for KG. |
| sled | Stagnant; migrate away. |
| **fjall** | v3 fast and competitive; serious sled/redb alternative. |
| HelixDB | Native graph+vector in Rust; track for v2 of MemPalace. |
| **rig** | Stable LLM provider abstraction; if MemPalace ever needs LLM calls beyond MCP, use this. |
| **swiftide** | Strong indexing pipeline; borrow ideas, don't depend. |
| langchain-rust | Useful for prototyping; not production. |
| llmchain-rs | Largely abandoned. |
| **fastembed-rs** | #1 shortlist. Replace Python ONNX. |
| candle | Use if you need full ML; otherwise fastembed-rs is enough. |
| ort | Wrap directly only for custom EPs. |
| burn | Watch; not yet production. |
| rust-bert | Avoid (libtorch). |
| **model2vec-rs** | Use for L1/wake-up; 500× faster, small accuracy cost. |
| mistral.rs | Overkill for embeddings; great for local LLM serving. |
| llama.cpp bindings | Good for GGUF embedding models on tiny disk budget. |
| **CocoIndex** | Best-in-class incremental ETL; consider as ingestion layer. |
| Memori | Architectural validation — keep using SQL, don't over-engineer. |
| Mem0 | #5 shortlist (architectural copy of fusion + ADD-only). |
| Letta / MemGPT | Borrow sleep-time consolidation pattern. |
| Graphiti / Zep | Borrow bi-temporal validity. |
| Basic Memory | Borrow Markdown export + Obsidian compat. |
| MCP server-memory (official) | Borrow tool naming for interoperability. |
| **A-MEM** | #4 shortlist. Borrow note construction + link generation + evolution. |
| Memvid | Single-file capsule idea; consider for `mpr export --format capsule`. |
| MemX | Sister project worth a code-read; identical Rust+SQL niche. |
| Vectorize Hindsight | Track; first open >90 LongMemEval. |
| Mastra Observational Memory | Closed; idea (observe-your-retrievals) already implemented in MemPalace's episodic feedback. |

---

## 8. Open questions for follow-up research

1. **Quantization curve for the palace at current size** — measure recall@10 on LongMemEval at f32 vs SQ8 vs PQ8x32. If SQ8 holds 96%+, the 4× memory savings free us to load the full palace into RAM on a laptop.
2. **A-MEM evolution latency budget** — at GPT-4o-mini cost (≈$0.0003/op), is it cheap enough to evolve the entire palace nightly? Or only the hot 1% (recently retrieved rooms)?
3. **Cross-language search quality** — fastembed-rs has `multilingual-e5-large` and `bge-m3`. Run a side-by-side LongMemEval-style probe in Russian/Portuguese to validate.
4. **MemPalace vs MemX** — the only other Rust+local long-term-memory system. A direct A/B on LongMemEval would settle architectural choices fast.
5. **Anthropic Memory Tool mapping** — would users prefer `mpr memory-tool` (filesystem façade) or `mpr mcp` (tool-call façade)? Probably both, but the file façade is much smaller code.

---

## Appendix A — License and activity snapshot

| Project | License | Activity heuristic |
|---------|---------|---------------------|
| fastembed-rs | Apache-2.0 | 901★, 250 commits, Apr 2026 release |
| tantivy | MIT | very high (Quickwit-backed) |
| usearch | Apache-2.0 | very high (Unum) |
| arroy / hannoy | MIT | high (Meilisearch) |
| hnsw_rs | MIT | medium |
| instant-distance | Apache-2.0 / MIT | medium-low (Nov 2025) |
| oasysdb | Apache-2.0 | low-medium |
| redb | MIT/Apache | high |
| fjall | MIT | high (v3 in 2025) |
| sled | MIT/Apache | low (stagnant) |
| HelixDB | MIT | medium-high |
| rig | MIT | very high (7.4k★, weekly releases) |
| swiftide | MIT | high (700★) |
| langchain-rust | MIT | medium |
| candle | Apache-2.0 | very high (HuggingFace) |
| model2vec-rs | MIT | high (MinishLab, 2026) |
| ort | Apache-2.0 | very high |
| CocoIndex | Apache-2.0 | very high (v1 Apr 2026) |
| Mem0 (Python) | Apache-2.0 | very high |
| Letta (Python) | Apache-2.0 | very high |
| Graphiti (Python) | Apache-2.0 | very high (Zep) |
| Basic Memory (Python) | varies | medium-high |
| A-MEM (research code) | research | NeurIPS 2025 |

## Appendix B — Sources

- arXiv:2502.12110 — A-MEM (Xu et al., NeurIPS 2025)
- arXiv:2504.19413 — Mem0 (Chhikara et al., ECAI 2025)
- arXiv:2504.13171 — Sleep-time Compute (Apr 2025)
- arXiv:2501.13956 — Zep / Graphiti (Jan 2025)
- arXiv:2603.16171 — MemX (Mar 2026, Rust)
- arXiv:2603.19935 — Memori (2026)
- arXiv:2605.12493 — LongMemEval-V2 / AgentRunbook (2026)
- arXiv:2605.08538 — Human-Inspired Memory Architecture (2026)
- arXiv:2304.03442 — Generative Agents (Park et al., 2023)
- mem0.ai/blog/state-of-ai-agent-memory-2026 — Mem0 2026 algorithm release
- mem0.ai/blog/mem0-the-token-efficient-memory-algorithm
- letta.com/blog/sleep-time-compute
- platform.claude.com/docs/en/agents-and-tools/tool-use/memory-tool
- anthropic.com/news/context-management
- anthropic.com/news/memory (2026-04-19, "Bringing memory to teams")
- github.com/agiresearch/A-MEM
- github.com/mem0ai/mem0
- github.com/letta-ai/letta
- github.com/getzep/graphiti
- github.com/basicmachines-co/basic-memory
- github.com/0xPlaygrounds/rig
- github.com/bosun-ai/swiftide
- github.com/Anush008/fastembed-rs
- github.com/MinishLab/model2vec-rs
- github.com/unum-cloud/usearch
- github.com/meilisearch/arroy
- github.com/quickwit-oss/tantivy
- github.com/cberner/redb
- github.com/fjall-rs/fjall
- github.com/HelixDB/helix-db
- github.com/cocoindex-io/cocoindex
- github.com/xiaowu0162/LongMemEval
- github.com/snap-research/locomo
- mem0.ai/blog/benchmarked-openai-memory-vs-langmem-vs-memgpt-vs-mem0-for-long-term-memory
- mastra.ai/research/observational-memory
- blog.alphasmanifesto.com/2026/04/11/a-mem-zettelkasten-for-agents/
- qdrant.tech/edge/ (Qdrant Edge, private beta)
