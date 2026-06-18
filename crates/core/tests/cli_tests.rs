// CLI integration tests — verify subcommand argument parsing via clap's
// programmatic API (`Cli::try_parse_from`). No `cargo run` subprocess overhead.
//
// Each test calls try_parse_from as if the binary received argv, then
// checks the result. Help-text assertions use clap's `CommandFactory`.

mod common;

use clap::CommandFactory;
use mempalace_core::cli::Cli;

// ---------------------------------------------------------------------------
// Help text verification
// ---------------------------------------------------------------------------

#[test]
fn test_cli_no_args_renders_help() {
    let cmd = Cli::command();
    let help = cmd.render_help().to_string();
    assert!(help.contains("MemPalace"), "help should contain 'MemPalace'");
    // Should list representative subcommands
    for sub in &["init", "search", "status", "actions", "diagnose", "serve", "mcp"] {
        assert!(help.contains(sub), "help should contain '{sub}'");
    }
}

#[test]
fn test_cli_help_flag_is_err() {
    // clap returns Err when --help is used (short-circuits to printing)
    assert!(Cli::try_parse_from(["mpr", "--help"]).is_err());
}

#[test]
fn test_cli_version_flag_is_err() {
    assert!(Cli::try_parse_from(["mpr", "--version"]).is_err());
}

// ---------------------------------------------------------------------------
// Valid invocations — every subcommand at least parses
// ---------------------------------------------------------------------------

#[test]
fn test_cli_init_parses_dir_arg() {
    assert!(Cli::try_parse_from(["mpr", "init", "/tmp/test_palace"]).is_ok());
}

#[test]
fn test_cli_init_parses_optional_flags() {
    assert!(Cli::try_parse_from([
        "mpr", "init", "/tmp/test_palace",
        "--yes", "--no-llm", "--auto-mine", "--lang", "en",
    ]).is_ok());
}

#[test]
fn test_cli_search_parses_query() {
    assert!(Cli::try_parse_from(["mpr", "search", "hello world"]).is_ok());
}

#[test]
fn test_cli_search_parses_optional_flags() {
    assert!(Cli::try_parse_from([
        "mpr", "search", "query",
        "--wing", "my_project",
        "--room", "dev",
        "--results", "10",
        "--fusion-mode", "hybrid",
        "--json",
    ]).is_ok());
}

#[test]
fn test_cli_mine_parses_dir() {
    assert!(Cli::try_parse_from(["mpr", "mine", "/tmp/some_project"]).is_ok());
}

#[test]
fn test_cli_mine_parses_optional_flags() {
    assert!(Cli::try_parse_from([
        "mpr", "mine", "/tmp/project",
        "--mode", "convos",
        "--wing", "chat",
        "--agent", "test-bot",
        "--limit", "100",
        "--dry-run", "--no-gitignore",
    ]).is_ok());
}

#[test]
fn test_cli_status_parses() {
    assert!(Cli::try_parse_from(["mpr", "status"]).is_ok());
}

#[test]
fn test_cli_serve_parses() {
    assert!(Cli::try_parse_from(["mpr", "serve"]).is_ok());
}

#[test]
fn test_cli_serve_parses_flags() {
    assert!(Cli::try_parse_from([
        "mpr", "serve", "--read-only", "--http", "--no-background",
    ]).is_ok());
}

#[test]
fn test_cli_diagnose_parses() {
    assert!(Cli::try_parse_from(["mpr", "diagnose"]).is_ok());
}

#[test]
fn test_cli_diagnose_deep_flag() {
    assert!(Cli::try_parse_from(["mpr", "diagnose", "--deep"]).is_ok());
}

#[test]
fn test_cli_actions_parses() {
    assert!(Cli::try_parse_from(["mpr", "actions"]).is_ok());
}

#[test]
fn test_cli_actions_status_filter() {
    assert!(Cli::try_parse_from(["mpr", "actions", "--status", "running", "--limit", "10"]).is_ok());
}

#[test]
fn test_cli_compress_parses() {
    assert!(Cli::try_parse_from(["mpr", "compress"]).is_ok());
}

#[test]
fn test_cli_split_parses_dir_arg() {
    assert!(Cli::try_parse_from(["mpr", "split", "/tmp/transcripts"]).is_ok());
}

#[test]
fn test_cli_connect_parses() {
    assert!(Cli::try_parse_from(["mpr", "connect"]).is_ok());
}

#[test]
fn test_cli_connect_adapter_arg() {
    assert!(Cli::try_parse_from(["mpr", "connect", "claude-code"]).is_ok());
}

#[test]
fn test_cli_export_parses_output_dir() {
    assert!(Cli::try_parse_from(["mpr", "export", "/tmp/export"]).is_ok());
}

#[test]
fn test_cli_frontier_parses() {
    assert!(Cli::try_parse_from(["mpr", "frontier"]).is_ok());
}

#[test]
fn test_cli_signals_parses_operation() {
    assert!(Cli::try_parse_from(["mpr", "signals", "list"]).is_ok());
}

#[test]
fn test_cli_forget_parses() {
    assert!(Cli::try_parse_from(["mpr", "forget", "--dry-run"]).is_ok());
}

#[test]
fn test_cli_evolve_parses() {
    assert!(Cli::try_parse_from(["mpr", "evolve", "--wing", "test", "--count", "5"]).is_ok());
}

#[test]
fn test_cli_context_parses() {
    assert!(Cli::try_parse_from(["mpr", "context"]).is_ok());
}

#[test]
fn test_cli_sessions_parses() {
    assert!(Cli::try_parse_from(["mpr", "sessions"]).is_ok());
}

#[test]
fn test_cli_vision_parses_query() {
    assert!(Cli::try_parse_from(["mpr", "vision", "find sunset image"]).is_ok());
}

#[test]
fn test_cli_wake_up_parses() {
    assert!(Cli::try_parse_from(["mpr", "wake-up"]).is_ok());
}

#[test]
fn test_cli_wake_up_wing_flag() {
    assert!(Cli::try_parse_from(["mpr", "wake-up", "--wing", "my_app"]).is_ok());
}

#[test]
fn test_cli_instructions_parses() {
    assert!(Cli::try_parse_from(["mpr", "instructions", "search"]).is_ok());
}

#[test]
fn test_cli_mine_device_parses() {
    assert!(Cli::try_parse_from(["mpr", "mine-device", "--dry-run"]).is_ok());
}

#[test]
fn test_cli_remove_parses() {
    assert!(Cli::try_parse_from(["mpr", "remove", "--force"]).is_ok());
}

#[test]
fn test_cli_demo_parses() {
    assert!(Cli::try_parse_from(["mpr", "demo"]).is_ok());
}

#[test]
fn test_cli_upgrade_parses() {
    assert!(Cli::try_parse_from(["mpr", "upgrade"]).is_ok());
}

#[test]
fn test_cli_stop_parses() {
    assert!(Cli::try_parse_from(["mpr", "stop"]).is_ok());
}

#[test]
fn test_cli_hook_parses() {
    assert!(Cli::try_parse_from([
        "mpr", "hook", "--hook", "session_end",
        "--data", r#"{"key": "value"}"#,
    ]).is_ok());
}

#[test]
fn test_cli_consolidate_parses() {
    assert!(Cli::try_parse_from(["mpr", "consolidate", "--dry-run"]).is_ok());
}

#[test]
fn test_cli_import_parses() {
    assert!(Cli::try_parse_from(["mpr", "import", "json", "/tmp/data.json"]).is_ok());
}

#[test]
fn test_cli_profile_parses() {
    assert!(Cli::try_parse_from(["mpr", "profile", "--wing", "my_app"]).is_ok());
}

#[test]
fn test_cli_mesh_parses() {
    assert!(Cli::try_parse_from(["mpr", "mesh", "--operation", "status"]).is_ok());
}

#[test]
fn test_cli_snapshot_parses() {
    assert!(Cli::try_parse_from(["mpr", "snapshot", "--name", "pre-upgrade"]).is_ok());
}

// ---------------------------------------------------------------------------
// Repair sub-subcommands
// ---------------------------------------------------------------------------

#[test]
fn test_cli_repair_scan() {
    assert!(Cli::try_parse_from(["mpr", "repair", "scan"]).is_ok());
}

#[test]
fn test_cli_repair_scan_wing() {
    assert!(Cli::try_parse_from(["mpr", "repair", "scan", "--wing", "my_project"]).is_ok());
}

#[test]
fn test_cli_repair_prune() {
    assert!(Cli::try_parse_from(["mpr", "repair", "prune"]).is_ok());
}

#[test]
fn test_cli_repair_prune_confirm() {
    assert!(Cli::try_parse_from(["mpr", "repair", "prune", "--confirm"]).is_ok());
}

#[test]
fn test_cli_repair_rebuild() {
    assert!(Cli::try_parse_from(["mpr", "repair", "rebuild"]).is_ok());
}

#[test]
fn test_cli_repair_cleanup_pid() {
    assert!(Cli::try_parse_from(["mpr", "repair", "cleanup-pid"]).is_ok());
}

#[test]
fn test_cli_repair_migrate_vector_index() {
    assert!(Cli::try_parse_from(["mpr", "repair", "migrate-vector-index"]).is_ok());
}

// ---------------------------------------------------------------------------
// Invalid invocations — should fail parsing
// ---------------------------------------------------------------------------

#[test]
fn test_cli_unknown_command_fails() {
    assert!(Cli::try_parse_from(["mpr", "does-not-exist"]).is_err());
}

#[test]
fn test_cli_init_missing_dir_fails() {
    assert!(Cli::try_parse_from(["mpr", "init"]).is_err());
}

#[test]
fn test_cli_search_missing_query_fails() {
    assert!(Cli::try_parse_from(["mpr", "search"]).is_err());
}

#[test]
fn test_cli_mine_missing_dir_fails() {
    assert!(Cli::try_parse_from(["mpr", "mine"]).is_err());
}

#[test]
fn test_cli_split_missing_dir_fails() {
    assert!(Cli::try_parse_from(["mpr", "split"]).is_err());
}

#[test]
fn test_cli_export_missing_output_dir_fails() {
    assert!(Cli::try_parse_from(["mpr", "export"]).is_err());
}

#[test]
fn test_cli_signals_missing_operation_fails() {
    assert!(Cli::try_parse_from(["mpr", "signals"]).is_err());
}

#[test]
fn test_cli_vision_missing_query_fails() {
    assert!(Cli::try_parse_from(["mpr", "vision"]).is_err());
}

#[test]
fn test_cli_instructions_missing_name_fails() {
    assert!(Cli::try_parse_from(["mpr", "instructions"]).is_err());
}

#[test]
fn test_cli_import_missing_args_fails() {
    assert!(Cli::try_parse_from(["mpr", "import"]).is_err());
}

// ---------------------------------------------------------------------------
// Fusion mode is a free string (no enum validation at parse time)
// ---------------------------------------------------------------------------

#[test]
fn test_cli_fusion_mode_accepts_any_string() {
    assert!(Cli::try_parse_from([
        "mpr", "search", "test", "--fusion-mode", "floob",
    ]).is_ok());
}

// ---------------------------------------------------------------------------
// Global --palace flag
// ---------------------------------------------------------------------------

#[test]
fn test_cli_palace_flag_parses() {
    assert!(Cli::try_parse_from(["mpr", "--palace", "/custom/palace", "status"]).is_ok());
}

#[test]
fn test_cli_palace_flag_with_serve() {
    assert!(Cli::try_parse_from(["mpr", "--palace", "/custom/palace", "serve"]).is_ok());
}

// ---------------------------------------------------------------------------
// Common helpers
// ---------------------------------------------------------------------------

#[test]
fn test_common_create_temp_palace_dir() {
    let (_dir, path) = common::create_temp_palace_dir("test_palace");
    assert!(path.exists(), "temp palace dir should exist");
    assert!(path.is_dir(), "temp palace path should be a directory");
}

#[test]
fn test_common_create_temp_palace_dir_isolation() {
    let (_dir1, path1) = common::create_temp_palace_dir("alpha");
    let (_dir2, path2) = common::create_temp_palace_dir("beta");
    assert_ne!(path1, path2, "two temp dirs should be isolated");
}
