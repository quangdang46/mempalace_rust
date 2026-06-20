## Description

The codebase has **124 calls** to `.lock().unwrap()` across 16 files, concentrated in:

| File | Count |
|---|---|
| `knowledge_graph.rs` | **58** |
| `palace_db.rs` | **25** |
| `config.rs` | **12** |
| `privacy.rs` | **5** |
| `profile.rs` | **3** |
| `llm_refine.rs` | **3** |
| `embed/openrouter_remote.rs` | **3** |
| `palace.rs` | **2** |
| `palace_graph.rs` | **2** |
| `embed/gemini_remote.rs` | **2** |
| `embed/cohere_remote.rs` | **1** |
| `embed/voyage_remote.rs` | **2** |
| `embed/openai_remote.rs` | **1** |
| `llm/openai_compat_provider.rs` | **2** |
| `llm/anthropic_provider.rs` | **2** |
| `background.rs` | **1** |

### Why this is a problem

Every `lock().unwrap()` poisons the Mutex/RwLock if any thread panics while holding the lock. Once poisoned, **every subsequent `lock().unwrap()` call panics** — rendering the entire subsystem permanently unavailable until the process restarts.

In `knowledge_graph.rs` (58 calls), a single panic in any KG operation (add_triple, query, invalidate) poisons the KG mutex, making every subsequent KG operation panic. This cascades through search, MCP tools, consolidation, sweeper, and any feature that touches the knowledge graph.

### Example problematic pattern

```rust
// knowledge_graph.rs (58 occurrences)
self.conn.lock().unwrap()

// palace_db.rs (25 occurrences)  
self.coordination.lock().unwrap()
```

### Fix

Replace `.lock().unwrap()` with `.lock().expect("descriptive message")` or `.lock().map_err(...)?` for production code paths. For optional/cancellable operations, use `.lock().unwrap_or_else(|e| ...)` to recover from poison by replacing the mutex content.

At minimum, critical paths should use `.lock().expect("kg mutex poisoned")` so failures are descriptive rather than silent cascading panics.
