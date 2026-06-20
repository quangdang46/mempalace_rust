## Description

`Config::load()` manually extracts only 9 fields from the config file using `serde_json::Value::get()` calls (lines 667-694), then fills the rest with `..Default::default()`. The Config struct has **30 fields** total, so **21 fields are silently dropped** on every load.

### Dropped fields

| Category | Dropped Fields |
|---|---|
| Search | `search_strategy`, `max_cache_size_mb` |
| Embedder | `embedder_identity_strict` |
| LLM config | `llm_provider`, `llm_model`, `llm_consent_given` |
| Feature flags | `consolidation_enabled`, `auto_compress`, `graph_extraction_enabled`, `rerank_enabled`, `snapshot_enabled`, `vision_enabled` |
| Budgets | `token_budget`, `max_obs_per_session` |
| Agent/Team | `agent_id`, `agent_scope`, `team_id`, `team_mode` |
| Fusion weights | `bm25_weight`, `vector_weight`, `graph_weight` |
| Misc | `max_backups`, `hooks_auto_save` |

### Impact

Any user setting to these 21 fields is silently replaced with defaults on every Config::load() call. For example:

- `search_strategy: "bm25"` in config → silently reset to `"fts5"` default
- `llm_provider`, `llm_model`, `token_budget` → **completely ignored**
- `consolidation_enabled`, `auto_compress`, `graph_extraction_enabled` → all reset to `None`

This affects all code paths that read config via `Config::load()`: cli.rs, mcp_server.rs, dedup.rs, closet_llm.rs, migrate.rs, palace_graph.rs, exporter.rs, fact_checker.rs, etc.

### Root Cause

The code uses hand-rolled field extraction on `serde_json::Value` instead of direct serde deserialization:

```rust
// Current (WRONG) — manual extraction, 21 fields lost
let content = std::fs::read_to_string(&config_path)?;
let file_config: serde_json::Value = serde_json::from_str(&content)?;
// ... manually extract 9 fields ...
Ok(Self { ..., ..Default::default() })
```

vs the correct approach:

```rust
// Should be (RIGHT) — serde handles all fields via #[serde(default)]
let content = std::fs::read_to_string(&config_path)?;
let config: Config = serde_json::from_str(&content)?;
Ok(config)
```

### Fix

Replace the manual extraction block with a single `serde_json::from_str::<Config>(&content)?` call. The struct already has `#[serde(default)]` annotations on all optional fields.
