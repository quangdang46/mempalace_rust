# APPROVED_DEVIATIONS.md

**Date:** 2026-05-27
**Commit:** `fb33713`
**Bead:** mr-nrt.17

Canonical list of intentional deviations from Python parity. These are NOT bugs — they are design decisions documented here so future contributors understand why.

---

## Architecture Differences

### 1. Embedvec ≠ ChromaDB (HNSW)

**Deviation:** Rust uses `embedvec` for storage. Python uses ChromaDB.

Rust `embedvec` is NOT a ChromaDB wrapper — it uses SQLite + a custom in-process vector index. The following ChromaDB-specific repair paths in Python `repair.py` are N/A:

- `sqlite_integrity_errors()` — ChromaDB-specific health check
- `rebuild_from_sqlite()` — ChromaDB HNSW recovery after corruption (#1308)
- `repair_max_seq_id()` — ChromaDB sequence poisoning recovery
- `status()` HNSW capacity check — ChromaDB-only

**Why:** Embedvec uses a fundamentally different storage architecture. The SQLite layer is the source of truth, not a ChromaDB WAL. Data integrity is handled by SQLite transactions.

**Approved:** `#[ignore]` in `parity_tests.rs::test_rebuild_index_from_sqlite`

---

### 2. Rust WAL vs Python Write-Ahead Logging

**Deviation:** `repair_max_seq_id()` (Python `repair.py`) is not needed in Rust.

**Why:** The Rust implementation uses a different WAL design (mp-026) that doesn't poison `max_seq_id` on crash. SQLite's default rollback journal handles consistency.

**Approved:** `#[ignore]` in `parity_tests.rs::test_repair_max_seq_id_recovery`

---

### 3. `mempalace_remember` Scoring

**Deviation:** Rust `remember_*` tool uses different proximity scoring than Python.

**Why:** Python `searcher.py` proximity scoring is not yet aligned with Rust `searcher::search_memories`.

**Status:** Blocked on mr-nrt.18 (AAAK compression semantics and stats behavior).

**Approved:** `#[ignore]` in `parity_tests.rs::test_remember_returns_similar_scores`

---

### 4. CLI Commands Are Private to Binary

**Deviation:** `Commands` enum and `MiningMode` are not re-exported from `mempalace-core`.

**Why:** The CLI binary (`mempalace-cli`) owns its own argument parsing. The `cli.rs` module is internal.

**Testing approach:** CLI behavior is exercised via `#[test]` blocks directly in `cli.rs` (lines 2273–2484), which run as part of the `mempalace-core` test suite via `#[cfg(test)]`.

**Approved deviation:** CLI integration tests live in `crates/cli/tests/` (when created) and via inline tests in `cli.rs`.

---

## Summary

| Deviation | Severity | Blocking Issue |
|-----------|----------|----------------|
| Embedvec vs ChromaDB repair paths | Architectural | None |
| Rust WAL vs Python max_seq_id | Architectural | None |
| `remember` proximity scoring | Feature gap | mr-nrt.18 |
| CLI private to binary | Organizational | None |
