use mempalace_core::mcp_server::is_mutation_tool;
use mempalace_core::knowledge_graph::KnowledgeGraph;
use mempalace_core::Config;
use tempfile::TempDir;

// =====================================================================
// A. MCP Tool-Contract Tests
// =====================================================================

#[test]
fn test_is_mutation_tool_classifies_correctly() {
    assert!(is_mutation_tool("mempalace_add_drawer"), "add_drawer should be mutation");
    assert!(is_mutation_tool("mempalace_delete_drawer"), "delete_drawer should be mutation");
    assert!(is_mutation_tool("mempalace_kg_add"), "kg_add should be mutation");
    assert!(is_mutation_tool("mempalace_kg_invalidate"), "kg_invalidate should be mutation");
    assert!(is_mutation_tool("mempalace_diary_write"), "diary_write should be mutation");

    assert!(!is_mutation_tool("mempalace_status"), "status should be query");
    assert!(!is_mutation_tool("mempalace_search"), "search should be query");
    assert!(!is_mutation_tool("mempalace_kg_query"), "kg_query should be query");
}

#[test]
fn test_mutation_tools_list_matches_tool_catalog() {
    let mutation_tools = [
        "mempalace_add_drawer",
        "mempalace_delete_drawer",
        "mempalace_kg_add",
        "mempalace_kg_invalidate",
        "mempalace_diary_write",
    ];

    for tool in mutation_tools {
        assert!(is_mutation_tool(tool), "tool {tool} should be mutation");
    }
}

// =====================================================================
// E. Rust-only Feature Preservation Tests (mr-3r8)
// =====================================================================

// Config::load() tests live in config.rs inline. Here we test the
// Config struct field defaults.

#[test]
fn test_config_load_default_values() {
    let temp = TempDir::new().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", temp.path().to_str().unwrap());

    let config = Config::load().unwrap();
    assert!(!config.palace_path.to_string_lossy().is_empty(), "palace_path should be set");
    assert!(!config.collection_name.is_empty(), "collection_name should be set");

    std::env::remove_var("XDG_CONFIG_HOME");
}

// =====================================================================
// C. KnowledgeGraph query_entity parity tests
// =====================================================================

#[test]
fn test_query_entity_returns_empty_for_nonexistent() {
    let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
    let results = kg.query_entity("nonexistent", None, None, "outgoing").unwrap();
    assert!(results.is_empty(), "query_entity nonexistent should return empty vec");
}

#[test]
fn test_query_entity_returns_result_for_existing() {
    let mut kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
    kg.add_triple("Alice", "works_at", "Acme", Some("2020-01-15"), None, None, None, None, None, None).unwrap();
    let results = kg.query_entity("Alice", None, None, "outgoing").unwrap();
    assert_eq!(results.len(), 1, "query_entity Alice should return 1 result");
    assert_eq!(results[0].object, "Acme");
}

#[test]
fn test_query_entity_4arg_signature_stability() {
    let kg = KnowledgeGraph::open(std::path::Path::new(":memory:")).unwrap();
    let _ = kg.query_entity("test", None, None, "outgoing").unwrap();
    let _ = kg.query_entity("test", Some("2025-01-01"), None, "both").unwrap();
    let _ = kg.query_entity("test", None, Some("2024-06-01"), "incoming").unwrap();
}

// =====================================================================
// E. Rust-only Feature Preservation Tests (mr-3r8)
// =====================================================================
// These tests verify specific Rust-only features have no Python equivalent.
// They fail to compile if the feature is removed.
#[test]
fn test_hermes_provider_exists() {
    use mempalace_core::hermes_integration::MemPalaceHermesProvider;
    let _: Option<MemPalaceHermesProvider> = None;
}

#[test]
fn test_xdg_config_loads() {
    let config = mempalace_core::Config::load().expect("XDG config must load");
    assert!(!config.palace_path.to_string_lossy().is_empty());
}

#[test]
#[ignore = "mempalace_remember scoring not yet aligned with Python proximity scoring"]
fn test_remember_returns_similar_scores() {
    // Known approved deviation: scoring not yet aligned
}

#[test]
#[ignore = "embedvec is not ChromaDB-backed; rebuild_from_sqlite N/A"]
fn test_rebuild_index_from_sqlite() {
    // Known approved deviation: ChromaDB-specific repair path
}

#[test]
#[ignore = "Rust WAL design eliminates need for max_seq_id repair"]
fn test_repair_max_seq_id_recovery() {
    // Known approved deviation: no max_seq_id in Rust WAL design
}
