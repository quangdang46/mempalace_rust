//! Regression tests for bugs fixed in the Oracle-identified bug-fixing session.
//!
//! Each test verifies that a specific bug pattern stays fixed:
//!
//! 1. `Config::load()` round-trips ALL fields (not just the 9 manually extracted)
//! 2. `safe_truncate` handles edge cases (CJK, emoji, empty, zero max_bytes)
//! 3. `ReservationMode::from_str` rejects invalid input instead of silently falling back
//! 4. Corrupted DB JSON fields cause `get_routine` / `get_run_status` to return `Err`

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};

fn serial_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

// ---------------------------------------------------------------------------
// 1. Config::load() round-trip — ALL fields preserved
// ---------------------------------------------------------------------------

#[test]
fn test_config_load_round_trip_all_fields() {
    let _guard = serial_env_lock().lock().unwrap_or_else(|e| e.into_inner());

    let temp = tempfile::tempdir().unwrap();
    let xdg = temp.path().to_str().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", xdg);

    // Build a JSON string with EVERY field set to a NON-default value.
    // We use the JSON approach because Config may be #[non_exhaustive].
    let json = serde_json::json!({
        "palace_path": "/custom/palace",
        "collection_name": "custom_collection",
        "people_map": { "alice": "Alice", "bob": "Robert" },
        "topic_wings": ["wing_a", "wing_b"],
        "hall_keywords": { "tech": ["rust"] },
        "embedding_model": "custom-model",
        "search_strategy": "bm25",
        "max_cache_size_mb": 512,
        "languages": ["en", "ja"],
        "llm_provider": "anthropic",
        "llm_model": "claude-opus-4",
        "consolidation_enabled": true,
        "auto_compress": false,
        "graph_extraction_enabled": true,
        "rerank_enabled": false,
        "snapshot_enabled": true,
        "vision_enabled": false,
        "token_budget": 32000,
        "max_obs_per_session": 500,
        "agent_id": "test-agent",
        "agent_scope": "project-x",
        "team_id": "team-42",
        "team_mode": true,
        "bm25_weight": 0.3,
        "vector_weight": 0.7,
        "graph_weight": 0.1,
        "llm_external_warn": false,
        "llm_consent_given": true,
        "max_backups": 5,
        "hooks_auto_save": false,
        "embedder_identity_strict": false
    });

    // Write to config file
    let config_path = mempalace_core::Config::config_file_path().unwrap();
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&config_path, serde_json::to_string_pretty(&json).unwrap()).unwrap();

    // Load it back via Config::load()
    let loaded = mempalace_core::Config::load().unwrap();

    // Verify EVERY field
    assert_eq!(loaded.palace_path.to_str().unwrap(), "/custom/palace");
    assert_eq!(loaded.collection_name, "custom_collection");
    assert_eq!(loaded.people_map.get("alice").unwrap(), "Alice");
    assert_eq!(loaded.people_map.get("bob").unwrap(), "Robert");
    assert_eq!(
        loaded.topic_wings,
        vec!["wing_a".to_string(), "wing_b".to_string()]
    );
    assert_eq!(
        loaded.hall_keywords.get("tech").unwrap(),
        &vec!["rust".to_string()]
    );
    assert_eq!(loaded.embedding_model, "custom-model");
    assert_eq!(loaded.search_strategy, "bm25");
    assert_eq!(loaded.max_cache_size_mb, 512);
    assert!(loaded.languages.contains(&"en".to_string()));
    assert!(loaded.languages.contains(&"ja".to_string()));
    assert_eq!(loaded.llm_provider.as_deref(), Some("anthropic"));
    assert_eq!(loaded.llm_model.as_deref(), Some("claude-opus-4"));
    assert_eq!(loaded.consolidation_enabled, Some(true));
    assert_eq!(loaded.auto_compress, Some(false));
    assert_eq!(loaded.graph_extraction_enabled, Some(true));
    assert_eq!(loaded.rerank_enabled, Some(false));
    assert_eq!(loaded.snapshot_enabled, Some(true));
    assert_eq!(loaded.vision_enabled, Some(false));
    assert_eq!(loaded.token_budget, Some(32000));
    assert_eq!(loaded.max_obs_per_session, Some(500));
    assert_eq!(loaded.agent_id.as_deref(), Some("test-agent"));
    assert_eq!(loaded.agent_scope.as_deref(), Some("project-x"));
    assert_eq!(loaded.team_id.as_deref(), Some("team-42"));
    assert_eq!(loaded.team_mode, Some(true));
    assert!((loaded.bm25_weight.unwrap() - 0.3).abs() < 1e-9);
    assert!((loaded.vector_weight.unwrap() - 0.7).abs() < 1e-9);
    assert!((loaded.graph_weight.unwrap() - 0.1).abs() < 1e-9);
    assert!(!loaded.llm_external_warn);
    assert!(loaded.llm_consent_given);
    assert_eq!(loaded.max_backups, Some(5));
    assert!(!loaded.hooks_auto_save);
    assert!(!loaded.embedder_identity_strict);

    std::env::remove_var("XDG_CONFIG_HOME");
}

#[test]
fn test_config_load_default_when_no_file() {
    let _guard = serial_env_lock().lock().unwrap_or_else(|e| e.into_inner());

    let temp = tempfile::tempdir().unwrap();
    let xdg = temp.path().to_str().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", xdg);

    // No config file exists => Config::load() returns Default
    let loaded = mempalace_core::Config::load().unwrap();
    assert_eq!(loaded.collection_name, "mempalace_drawers");
    assert_eq!(loaded.embedding_model, "naive");

    std::env::remove_var("XDG_CONFIG_HOME");
}

// ---------------------------------------------------------------------------
// 2. safe_truncate — edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_safe_truncate_ascii() {
    assert_eq!(
        mempalace_core::normalize::safe_truncate("hello world", 5),
        "hello"
    );
}

#[test]
fn test_safe_truncate_cjk() {
    // Each CJK character is 3 bytes in UTF-8
    let s = "你好世界"; // 12 bytes
                        // Truncate to 6 bytes => should give "你好" (6 bytes, 2 chars)
    assert_eq!(mempalace_core::normalize::safe_truncate(s, 6), "你好");
    // Truncate to 7 bytes => 你好 + partial of third char → must back up to char boundary
    let truncated = mempalace_core::normalize::safe_truncate(s, 7);
    assert_eq!(truncated, "你好");
}

#[test]
fn test_safe_truncate_emoji() {
    // Emoji can be 4 bytes in UTF-8
    let s = "a😀b"; // 'a' (1) + 😀 (4) + 'b' (1) = 6 bytes
    assert_eq!(mempalace_core::normalize::safe_truncate(s, 1), "a");
    // Truncate at 5 bytes => 'a' + 😀 = 5 bytes exactly
    assert_eq!(mempalace_core::normalize::safe_truncate(s, 5), "a😀");
    // Truncate at 2 bytes => only 'a' (can't split 😀)
    assert_eq!(mempalace_core::normalize::safe_truncate(s, 2), "a");
}

#[test]
fn test_safe_truncate_empty() {
    assert_eq!(mempalace_core::normalize::safe_truncate("", 10), "");
}

#[test]
fn test_safe_truncate_zero_max_bytes() {
    assert_eq!(mempalace_core::normalize::safe_truncate("hello", 0), "");
}

#[test]
fn test_safe_truncate_max_bytes_exceeds_len() {
    assert_eq!(
        mempalace_core::normalize::safe_truncate("hello", 100),
        "hello"
    );
}

// ---------------------------------------------------------------------------
// 3. ReservationMode::from_str — reject invalid input
// ---------------------------------------------------------------------------

#[test]
fn test_reservation_mode_from_str_valid() {
    use mempalace_core::coordination::file_reservations::ReservationMode;

    assert_eq!(
        ReservationMode::from_str("exclusive").unwrap(),
        ReservationMode::Exclusive
    );
    assert_eq!(
        ReservationMode::from_str("Exclusive").unwrap(),
        ReservationMode::Exclusive
    );
    assert_eq!(
        ReservationMode::from_str("shared").unwrap(),
        ReservationMode::Shared
    );
    assert_eq!(
        ReservationMode::from_str("SHARED").unwrap(),
        ReservationMode::Shared
    );
    assert_eq!(
        ReservationMode::from_str("non_exclusive").unwrap(),
        ReservationMode::Shared
    );
    assert_eq!(
        ReservationMode::from_str("observe").unwrap(),
        ReservationMode::Shared
    );
    assert_eq!(
        ReservationMode::from_str("read").unwrap(),
        ReservationMode::Shared
    );
}

#[test]
fn test_reservation_mode_from_str_invalid() {
    use mempalace_core::coordination::file_reservations::ReservationMode;

    let err = ReservationMode::from_str("invalid").unwrap_err();
    assert!(
        err.to_string().contains("invalid"),
        "error message should contain 'invalid', got: {}",
        err
    );
    assert!(
        err.to_string().contains("reservation mode"),
        "error message should mention 'reservation mode', got: {}",
        err
    );

    // Empty string should also fail
    assert!(ReservationMode::from_str("").is_err());

    // Random garbage should fail
    assert!(ReservationMode::from_str("exklusive").is_err());
    assert!(ReservationMode::from_str("sharred").is_err());
}

// ---------------------------------------------------------------------------
// 4. Corrupted DB JSON fields => Err (not silent fallback)
// ---------------------------------------------------------------------------

/// Helper: open a RoutineStore on a temp file, then corrupt a column via raw SQL.
fn corrupt_and_verify(
    field: &str,
    bad_value: &str,
    validate: fn(&mempalace_core::coordination::routines::RoutineStore) -> bool,
) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("routines.db");

    // Open the store, create a routine, then close
    {
        use chrono::Utc;
        use mempalace_core::coordination::routines::RoutineStore;
        use mempalace_core::types::Routine;

        let store = RoutineStore::open(&db_path).unwrap();
        let routine = Routine {
            id: "r-1".to_string(),
            name: "test".to_string(),
            description: "".to_string(),
            steps: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
            frozen: false,
            tags: vec![],
            source_procedural_ids: vec![],
        };
        store.create_routine(&routine).unwrap();
    }
    // Drop store so the connection is closed

    // Corrupt the field via a raw SQLite connection
    {
        let raw = rusqlite::Connection::open(&db_path).unwrap();
        raw.execute(
            &format!("UPDATE routines SET {field} = ?1 WHERE id = 'r-1'"),
            rusqlite::params![bad_value],
        )
        .unwrap();
    }

    // Re-open and verify that get_routine returns an error
    {
        use mempalace_core::coordination::routines::RoutineStore;
        let store = RoutineStore::open(&db_path).unwrap();
        let is_err = validate(&store);
        assert!(is_err, "corrupted {field} should produce Err");
    }
}

#[test]
fn test_corrupted_db_json_in_routine_steps_returns_err() {
    corrupt_and_verify("steps", "NOT_VALID_JSON", |store| {
        store.get_routine("r-1").is_err()
    });
}

#[test]
fn test_corrupted_db_json_in_routine_tags_returns_err() {
    corrupt_and_verify("tags", "{{{BROKEN_JSON", |store| {
        store.get_routine("r-1").is_err()
    });
}

#[test]
fn test_corrupted_db_json_in_routine_created_at_returns_err() {
    corrupt_and_verify("created_at", "not-a-date", |store| {
        store.get_routine("r-1").is_err()
    });
}

#[test]
fn test_corrupted_db_json_in_routine_updated_at_returns_err() {
    corrupt_and_verify("updated_at", "also-not-a-date", |store| {
        store.get_routine("r-1").is_err()
    });
}

#[test]
fn test_corrupted_db_json_in_routine_source_procedural_ids_returns_err() {
    corrupt_and_verify("source_procedural_ids", "[broken", |store| {
        store.get_routine("r-1").is_err()
    });
}
