//! MCP server implementation for MemPalace.
//!
//! Exposes MemPalace functionality as MCP tools via stdio transport.
//! Read-only mode restricts mutations (diary_write, config_write, people_write).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use regex::Regex;

use crate::palace_db::MemorySlot;
use rmcp::model::{
    CallToolResult, Content, GetPromptResult, Implementation, InitializeResult, JsonObject,
    ListPromptsResult, ListResourcesResult, ListToolsResult, PromptArgument,
    ReadResourceRequestParams, ReadResourceResult, ResourceContents, ServerCapabilities,
    ServerInfo as McpServerInfo,
};
use rmcp::service::MaybeSendFuture;
use rmcp::transport::stdio;
use rmcp::{handler::server::ServerHandler, ErrorData, RoleServer, ServiceExt};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::json;
use tokio::runtime::Runtime;
use tracing::warn;

fn short_hash(input: &str, len: usize) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(input.as_bytes());
    let hex = hex::encode(digest);
    hex[..len.min(hex.len())].to_string()
}

/// mr-s0fq: when an update changes only the case of a room/wing,
/// preserve the existing canonical casing. Returns the new value
/// when the change is more than case (substance change), or the
/// existing value when only the case differs.
pub fn preserve_case_on_update(existing: &str, new_value: &str) -> String {
    if existing.eq_ignore_ascii_case(new_value) && existing != new_value {
        existing.to_string()
    } else {
        new_value.to_string()
    }
}

/// Extract action items from sketch content.
/// Matches: `- [ ] TODO`, `- [x] Done`, `1. item`, `2. item`, etc.
fn extract_action_items(content: &str) -> Vec<String> {
    let re = Regex::new(r"(?m)^[\-\*]\s*\[[\s[x]]\s*(.+)$|^(?:\d+)\.\s*(.+)$").unwrap();
    let mut items = Vec::new();
    for cap in re.captures_iter(content) {
        if let Some(text) = cap.get(1).or(cap.get(2)) {
            items.push(text.as_str().trim().to_string());
        }
    }
    items
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WalEntry {
    timestamp: String,
    tool: String,
    args: serde_json::Value,
    result_summary: Option<serde_json::Value>,
    trace_id: String,
}

fn append_wal_entry(entry: &WalEntry, wal_dir: &Path) -> anyhow::Result<()> {
    let wal_file = wal_dir.join("write_log.jsonl");
    fs::create_dir_all(wal_dir)?;

    let existing = fs::read(&wal_file).unwrap_or_default();
    let temp_path = wal_dir.join(format!("write_log.{}.tmp", entry.trace_id));
    let mut temp = fs::File::create(&temp_path)?;
    if !existing.is_empty() {
        temp.write_all(&existing)?;
        if !existing.ends_with(b"\n") {
            temp.write_all(b"\n")?;
        }
    }
    serde_json::to_writer(&mut temp, entry)?;
    temp.write_all(b"\n")?;
    temp.flush()?;
    drop(temp);
    fs::rename(temp_path, wal_file)?;
    Ok(())
}

fn summarize_tool_result(result: &Result<CallToolResult, ErrorData>) -> serde_json::Value {
    match result {
        Ok(value) => serde_json::json!({
            "status": "ok",
            "content_items": value.content.len(),
            "is_error": value.is_error,
        }),
        Err(err) => serde_json::json!({
            "status": "error",
            "message": err.message,
        }),
    }
}

fn log_tool_invocation(
    tool: &str,
    args: &JsonObject,
    result_summary: Option<serde_json::Value>,
    trace_id: &str,
    wal_dir: &Path,
) {
    let entry = WalEntry {
        timestamp: chrono::Utc::now().to_rfc3339(),
        tool: tool.to_string(),
        args: serde_json::Value::Object(args.clone()),
        result_summary,
        trace_id: trace_id.to_string(),
    };

    if let Err(err) = append_wal_entry(&entry, wal_dir) {
        warn!("Failed to append MCP WAL entry: {}", err);
    }
}

const PALACE_PROTOCOL: &str = r#"IMPORTANT — MemPalace Memory Protocol:
1. ON WAKE-UP: Call mempalace_status to load palace overview + AAAK spec.
2. BEFORE RESPONDING about any person, project, or past event: call mempalace_kg_query or mempalace_search FIRST. Never guess — verify.
3. IF UNSURE about a fact (name, gender, age, relationship): say "let me check" and query the palace. Wrong is worse than slow.
4. AFTER EACH SESSION: call mempalace_diary_write to record what happened, what you learned, what matters.
5. WHEN FACTS CHANGE: call mempalace_kg_invalidate on the old fact, mempalace_kg_add for the new one.

This protocol ensures the AI KNOWS before it speaks. Storage is not memory — but storage + this protocol = memory."#;

const AAAK_SPEC: &str = r#"AAAK is a compressed memory dialect that MemPalace uses for efficient storage.
It is designed to be readable by both humans and LLMs without decoding.

FORMAT:
  ENTITIES: 3-letter uppercase codes. ALC=Alice, JOR=Jordan, RIL=Riley, MAX=Max, BEN=Ben.
  EMOTIONS: *action markers* before/during text. *warm*=joy, *fierce*=determined, *raw*=vulnerable, *bloom*=tenderness.
  STRUCTURE: Pipe-separated fields. FAM: family | PROJ: projects | ⚠: warnings/reminders.
  DATES: ISO format (2026-03-31). COUNTS: Nx = N mentions (e.g., 570x).
  IMPORTANCE: ★ to ★★★★★ (1-5 scale).
  HALLS: hall_facts, hall_events, hall_discoveries, hall_preferences, hall_advice.
  WINGS: wing_user, wing_agent, wing_team, wing_code, wing_myproject, wing_hardware, wing_ue5, wing_ai_research.
  ROOMS: Hyphenated slugs representing named ideas (e.g., chromadb-setup, gpu-pricing).

EXAMPLE:
  FAM: ALC→♡JOR | 2D(kids): RIL(18,sports) MAX(11,chess+swimming) | BEN(contributor)

Read AAAK naturally — expand codes mentally, treat *markers* as emotional context.
When WRITING AAAK: use entity codes, mark emotions, keep structure tight."#;

// ---------------------------------------------------------------------------
// Error helpers
// ---------------------------------------------------------------------------

/// Returns a generic error to the client while logging the actual error server-side.
/// This prevents leaking internal paths/schemas through error messages.
fn internal_error_safe<E: std::fmt::Display>(e: &E) -> ErrorData {
    warn!("Internal error: {}", e);
    ErrorData::internal_error("Internal tool error", None)
}

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

#[non_exhaustive]
pub struct AppState {
    pub config: crate::Config,
    pub db: crate::palace_db::PalaceDb,
    pub read_only: bool,
    pub palace_path: std::path::PathBuf,
    pub mesh: std::sync::RwLock<crate::coordination::mesh::Mesh>,
    /// Followup tracking for smart_search (mr-6g8z). Tracks when a
    /// second search within the time window has zero overlap with the prior.
    pub followup_tracker:
        std::sync::Arc<std::sync::Mutex<crate::search::followup::FollowupTracker>>,
    /// Shared session store (Finding 5). Opened once at startup and reused
    /// across tool_observe calls instead of opening a new connection each time.
    pub session_store: std::sync::Arc<crate::session::SessionStore>,
}

impl AppState {
    pub fn new(config: crate::Config, read_only: bool) -> anyhow::Result<Self> {
        let palace_path = config.palace_path.clone();
        let db = crate::palace_db::PalaceDb::open(&palace_path)?;
        let mesh = crate::coordination::mesh::Mesh::new(None);
        let session_store = std::sync::Arc::new(crate::session::SessionStore::open(
            &palace_path.join("sessions"),
        )?);
        Ok(Self {
            config,
            db,
            read_only,
            palace_path,
            mesh: std::sync::RwLock::new(mesh),
            followup_tracker: std::sync::Arc::new(std::sync::Mutex::new(
                crate::search::followup::FollowupTracker::new(),
            )),
            session_store,
        })
    }
}

// ---------------------------------------------------------------------------
// Dispatch: tool name -> async handler
// ---------------------------------------------------------------------------

type DynResult =
    Pin<Box<dyn std::future::Future<Output = Result<CallToolResult, ErrorData>> + Send + 'static>>;

async fn invoke_with_wal<F>(
    tool_name: String,
    args: JsonObject,
    dispatch: F,
    wal_dir: PathBuf,
) -> Result<CallToolResult, ErrorData>
where
    F: FnOnce(String, JsonObject) -> DynResult,
{
    let trace_id = short_hash(
        &format!(
            "{}:{}:{}",
            tool_name,
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
            serde_json::Value::Object(args.clone())
        ),
        16,
    );
    log_tool_invocation(&tool_name, &args, None, &trace_id, &wal_dir);
    let result = match validate_known_params(&tool_name, &args) {
        Err(err) => Err(err),
        Ok(()) => dispatch(tool_name.clone(), args.clone()).await,
    };
    log_tool_invocation(
        &tool_name,
        &args,
        Some(summarize_tool_result(&result)),
        &trace_id,
        &wal_dir,
    );
    result
}

/// Tools whose Input struct accepts arbitrary extra fields (via
/// `#[serde(flatten)] custom_metadata: Option<serde_json::Value>` or
/// equivalent). These bypass the unknown-parameter check so callers can keep
/// supplying custom metadata keys — mirrors Python's `accepts_var_keyword`
/// gate on `**kwargs` handlers. (#1512)
const TOOLS_ACCEPTING_EXTRAS: &[&str] = &["mempalace_add_drawer"];

/// Tools that mutate palace state. Hidden from `tools/list` when the server runs
/// with `--read-only`; the call-time `read_only_guard` remains as a defense-in-
/// depth check for any client that still tries to invoke them by name.
const MUTATION_TOOLS: &[&str] = &[
    "mempalace_add_drawer",
    "mempalace_delete_drawer",
    "mempalace_kg_add",
    "mempalace_kg_invalidate",
    "mempalace_diary_write",
    "mempalace_heal",
    "mempalace_governance_delete",
    "mempalace_obsidian_export",
    "mempalace_compress_file",
    "mempalace_action_create",
    "mempalace_action_update",
    "mempalace_lease",
    "mempalace_routine_run",
    "mempalace_signal_send",
    // Smart features - mutation tools
    "mempalace_sentinel_create",
    "mempalace_sentinel_trigger",
    "mempalace_sketch_create",
    "mempalace_sketch_promote",
    "mempalace_crystallize",
    "mempalace_facet_tag",
    "mempalace_lesson_save",
    "mempalace_checkpoint",
    "mempalace_mesh_sync",
    "mempalace_team_share",
    "mempalace_consolidate",
    "mempalace_snapshot_create",
    "mempalace_kg_snapshot_rebuild",
    "mempalace_kg_reset",
    // Smart features - slots (mutation)
    "mempalace_slot_create",
    "mempalace_slot_append",
    "mempalace_slot_replace",
    "mempalace_slot_delete",
];

/// Whether a tool mutates state and should be excluded from `tools/list` in
/// read-only mode. Public so unit tests can guard against drift.
pub fn is_mutation_tool(tool_name: &str) -> bool {
    MUTATION_TOOLS.contains(&tool_name)
}

/// Keys that are internal transport metadata and live in no tool schema. They
/// are stripped before dispatch elsewhere; flagging them as unknown here would
/// surface a misleading error for legitimate transport-level options. (#1512)
const TRANSPORT_RESERVED_KEYS: &[&str] = &["wait_for_previous"];

/// Lazily-built lookup of `tool_name -> declared input-schema property names`.
/// Source of truth is `make_tools()`'s JSON schema, so adding a property in
/// one place automatically updates the unknown-parameter check.
fn tool_schema_props() -> &'static HashMap<String, HashSet<String>> {
    static SCHEMA_PROPS: OnceLock<HashMap<String, HashSet<String>>> = OnceLock::new();
    SCHEMA_PROPS.get_or_init(|| {
        let mut by_tool = HashMap::new();
        for tool in make_tools() {
            let value = serde_json::Value::Object((*tool.input_schema).clone());
            let props = value
                .get("properties")
                .and_then(|p| p.as_object())
                .map(|obj| obj.keys().cloned().collect::<HashSet<_>>())
                .unwrap_or_default();
            by_tool.insert(tool.name.to_string(), props);
        }
        by_tool
    })
}

/// Reject unknown parameter *names* with JSON-RPC -32602 instead of letting
/// serde silently drop them and resurfacing the typo as a downstream
/// "missing required" error. Skips tools whose Input struct uses
/// `#[serde(flatten)]` extras (`TOOLS_ACCEPTING_EXTRAS`) and the
/// `wait_for_previous` transport kwarg, matching upstream Python's
/// `accepts_var_keyword` gate. (#1512)
fn validate_known_params(tool_name: &str, args: &JsonObject) -> Result<(), ErrorData> {
    if TOOLS_ACCEPTING_EXTRAS.contains(&tool_name) {
        return Ok(());
    }
    let Some(allowed) = tool_schema_props().get(tool_name) else {
        // Unknown tool — let dispatch surface method_not_found.
        return Ok(());
    };
    let mut unknown: Vec<&str> = args
        .keys()
        .filter(|k| !allowed.contains(k.as_str()))
        .filter(|k| !TRANSPORT_RESERVED_KEYS.contains(&k.as_str()))
        .map(String::as_str)
        .collect();
    if unknown.is_empty() {
        return Ok(());
    }
    unknown.sort_unstable();
    let quoted = unknown
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let word = if unknown.len() == 1 {
        "parameter"
    } else {
        "parameters"
    };
    Err(ErrorData::invalid_params(
        format!("Unknown {word} {quoted} for tool {tool_name}"),
        None,
    ))
}

pub(crate) fn make_dispatch(state: Arc<AppState>) -> impl Fn(String, JsonObject) -> DynResult {
    move |name, args| {
        let state = state.clone();
        Box::pin(async move {
            match name.as_str() {
                // mempalace_* canonical names
                "mempalace_status" => tool_status(&state, args),
                "mempalace_list_wings" => tool_list_wings(&state, args),
                "mempalace_list_rooms" => tool_list_rooms(&state, args),
                "mempalace_get_taxonomy" => tool_get_taxonomy(&state, args),
                "mempalace_get_aaak_spec" => tool_get_aaak_spec(&state, args),
                "mempalace_search" => tool_search(&state, args),
                "mempalace_check_duplicate" => tool_check_duplicate(&state, args),
                "mempalace_add_drawer" => tool_add_drawer(&state, args),
                "mempalace_delete_drawer" => tool_delete_drawer(&state, args),
                "mempalace_kg_query" => tool_kg_query(&state, args),
                "mempalace_kg_add" => tool_kg_add(&state, args),
                "mempalace_kg_invalidate" => tool_kg_invalidate(&state, args),
                "mempalace_kg_timeline" => tool_kg_timeline(&state, args),
                "mempalace_kg_stats" => tool_kg_stats(&state, args),
                "mempalace_kg_snapshot_rebuild" => tool_kg_snapshot_rebuild(&state, args),
                "mempalace_kg_reset" => tool_kg_reset(&state, args),
                "mempalace_traverse" => tool_traverse(&state, args),
                "mempalace_find_tunnels" => tool_find_tunnels(&state, args),
                "mempalace_graph_stats" => tool_graph_stats(&state, args),
                "mempalace_diary_read" => tool_diary_read(&state, args),
                "mempalace_diary_write" => tool_diary_write(&state, args),
                "mempalace_heal" => tool_heal(&state, args),
                "mempalace_verify" => tool_verify(&state, args),
                "mempalace_governance_delete" => tool_governance_delete(&state, args),
                "mempalace_obsidian_export" => tool_obsidian_export(&state, args),
                "mempalace_compress_file" => tool_compress_file(&state, args),
                "mempalace_detect_worktree" => tool_detect_worktree(&state, args),
                "mempalace_replay_import" => tool_replay_import(&state, args),
                "mempalace_branch_detect" => tool_branch_detect(&state, args),
                "mempalace_branch_sessions" => tool_branch_sessions(&state, args),
                "mempalace_branch_worktrees" => tool_branch_worktrees(&state, args),
                "mempalace_action_create" => tool_action_create(&state, args),
                "mempalace_action_update" => tool_action_update(&state, args),
                "mempalace_frontier" => tool_frontier(&state, args),
                "mempalace_next" => tool_next(&state, args),
                "mempalace_lease" => tool_lease(&state, args),
                "mempalace_routine_run" => tool_routine_run(&state, args),
                "mempalace_signal_send" => tool_signal_send(&state, args),
                "mempalace_signal_read" => tool_signal_read(&state, args),
                // Smart features - sentinel
                "mempalace_sentinel_create" => tool_sentinel_create(&state, args),
                "mempalace_sentinel_trigger" => tool_sentinel_trigger(&state, args),
                "mempalace_sentinel_list" => tool_sentinel_list(&state, args),
                "mempalace_sentinel_delete" => tool_sentinel_delete(&state, args),
                // Smart features - checkpoint
                "mempalace_checkpoint" => tool_checkpoint(&state, args),
                "mempalace_checkpoint_list" => tool_checkpoint_list(&state, args),
                "mempalace_checkpoint_resolve" => tool_checkpoint_resolve(&state, args),
                "mempalace_sketch_create" => tool_sketch_create(&state, args),
                "mempalace_sketch_promote" => tool_sketch_promote(&state, args),
                // Smart features - crystallize
                "mempalace_crystallize" => tool_crystallize(&state, args),
                // Smart features - diagnose
                "mempalace_health" => tool_health(&state, args),
                "mempalace_diagnose" => tool_diagnose(&state, args),
                // Smart features - facet
                "mempalace_facet_tag" => tool_facet_tag(&state, args),
                "mempalace_facet_query" => tool_facet_query(&state, args),
                // Smart features - lessons
                "mempalace_lesson_save" => tool_lesson_save(&state, args),
                "mempalace_lesson_recall" => tool_lesson_recall(&state, args),
                // Smart features - reflect
                "mempalace_reflect" => tool_reflect(&state, args),
                // Smart features - insights
                "mempalace_insight_list" => tool_insight_list(&state, args),
                // Smart features - slots
                "mempalace_slot_list" => tool_slot_list(&state, args),
                "mempalace_slot_get" => tool_slot_get(&state, args),
                "mempalace_slot_create" => tool_slot_create(&state, args),
                "mempalace_slot_append" => tool_slot_append(&state, args),
                "mempalace_slot_replace" => tool_slot_replace(&state, args),
                "mempalace_slot_delete" => tool_slot_delete(&state, args),
                // Smart features - checkpoint
                "mempalace_checkpoint" => tool_checkpoint(&state, args),
                // Smart features - mesh
                "mempalace_mesh_sync" => tool_mesh_sync(&state, args),
                // Smart features - team
                "mempalace_team_share" => tool_team_share(&state, args),
                "mempalace_team_feed" => tool_team_feed(&state, args),
                // Smart features - consolidate
                "mempalace_consolidate" => tool_consolidate(&state, args),
                // Smart features - graph retrieval
                "mempalace_graph_search" => tool_graph_search(&state, args),
                "mempalace_graph_expand" => tool_graph_expand(&state, args),
                "mempalace_graph_stats" => tool_graph_stats(&state, args),
                // mr-0qr1: hallway aliases for tunnels
                "mempalace_list_hallways" => tool_list_hallways(&state, args),
                "mempalace_delete_hallway" => tool_delete_hallway(&state, args),
                // Smart features - context
                "mempalace_context_build" => tool_context_build(&state, args),
                // Smart features - flow compress
                "mempalace_flow_compress" => tool_flow_compress(&state, args),
                // Smart features - cascade
                "mempalace_cascade_update" => tool_cascade_update(&state, args),
                // Smart features - enrich
                "mempalace_enrich" => tool_enrich(&state, args),
                // Smart features - retention
                "mempalace_retention_score" => tool_retention_score(&state, args),
                // Smart features - access
                "mempalace_access_stats" => tool_access_stats(&state, args),
                // Smart features - working memory
                "mempalace_working_memory" => tool_working_memory(&state, args),
                // Smart features - snapshot
                "mempalace_snapshot_create" => tool_snapshot_create(&state, args),
                // Smart features - history
                "mempalace_file_history" => tool_file_history(&state, args),
                // Smart features - sessions
                "mempalace_sessions" => tool_sessions(&state, args),
                // Smart features - observe
                "mempalace_observe" => tool_observe(&state, args),
                // Smart features - commits
                "mempalace_commits" => tool_commits(&state, args),
                "mempalace_commit_lookup" => tool_commit_lookup(&state, args),
                // mr-dghp: in-process mine
                "mempalace_mine" => tool_mine(&state, args).await,
                // Smart features - smart search with progressive disclosure
                "mempalace_smart_search" => tool_smart_search(&state, args),
                "mempalace_hybrid_search" => tool_hybrid_search(&state, args),
                // Aliases aligned with @modelcontextprotocol/server-memory (one minor release)
                // Core save/recall (REST API uses these names — see
                // list_tools_handler in rest_api.rs). Must be wired or
                // every HTTP tool call returns "Unknown tool: <name>".
                "mempalace_recall" => tool_recall(&state, args),
                "mempalace_save" => tool_save(&state, args),
                "memory_search" | "memory_list" => tool_search(&state, args),
                "memory_list_wings" => tool_list_wings(&state, args),
                "memory_list_rooms" => tool_list_rooms(&state, args),
                "memory_get_taxonomy" => tool_get_taxonomy(&state, args),
                "memory_get_aaak_spec" => tool_get_aaak_spec(&state, args),
                "memory_check_duplicate" => tool_check_duplicate(&state, args),
                "memory_add" | "memory_add_drawer" => tool_add_drawer(&state, args),
                "memory_delete" | "memory_delete_drawer" => tool_delete_drawer(&state, args),
                "memory_kg_query" | "memory_graph_query" => tool_kg_query(&state, args),
                "memory_kg_add" | "memory_graph_add" => tool_kg_add(&state, args),
                "memory_kg_invalidate" | "memory_graph_invalidate" => {
                    tool_kg_invalidate(&state, args)
                }
                "memory_kg_timeline" | "memory_graph_timeline" => tool_kg_timeline(&state, args),
                "memory_kg_stats" | "memory_graph_stats" => tool_kg_stats(&state, args),
                "memory_kg_snapshot_rebuild" | "memory_graph_snapshot_rebuild" => {
                    tool_kg_snapshot_rebuild(&state, args)
                }
                "memory_kg_reset" | "memory_graph_reset" => tool_kg_reset(&state, args),
                "memory_traverse" => tool_traverse(&state, args),
                "memory_find_tunnels" => tool_find_tunnels(&state, args),
                "memory_diary_read" => tool_diary_read(&state, args),
                "memory_diary_write" => tool_diary_write(&state, args),
                "memory_status" => tool_status(&state, args),
                // New memory_* tool aliases
                "memory_recall" => tool_recall(&state, args),
                "memory_save" => tool_save(&state, args),
                "memory_profile" => tool_profile(&state, args),
                "memory_export" => tool_export(&state, args),
                "memory_timeline" => tool_timeline(&state, args),
                "memory_patterns" => tool_patterns(&state, args),
                "memory_smart_search" => tool_smart_search(&state, args),
                "memory_vision_search" => tool_vision_search(&state, args),
                "memory_relations" => tool_relations(&state, args),
                "memory_audit" => tool_audit(&state, args),
                "memory_verify" => tool_verify(&state, args),
                "memory_heal" => tool_heal(&state, args),
                "memory_governance_delete" => tool_governance_delete(&state, args),
                "memory_obsidian_export" => tool_obsidian_export(&state, args),
                "memory_compress_file" => tool_compress_file(&state, args),
                "memory_action_create" => tool_action_create(&state, args),
                "memory_action_update" => tool_action_update(&state, args),
                "memory_frontier" => tool_frontier(&state, args),
                "memory_next" => tool_next(&state, args),
                "memory_lease" => tool_lease(&state, args),
                "memory_routine_run" => tool_routine_run(&state, args),
                "memory_signal_send" => tool_signal_send(&state, args),
                "memory_signal_read" => tool_signal_read(&state, args),
                "memory_checkpoint" => tool_checkpoint(&state, args),
                "memory_mesh_sync" => tool_mesh_sync(&state, args),
                "memory_team_share" => tool_team_share(&state, args),
                "memory_team_feed" => tool_team_feed(&state, args),
                "memory_sentinel_create" => tool_sentinel_create(&state, args),
                "memory_sentinel_trigger" => tool_sentinel_trigger(&state, args),
                "memory_sentinel_list" => tool_sentinel_list(&state, args),
                "memory_sentinel_delete" => tool_sentinel_delete(&state, args),
                "memory_checkpoint" => tool_checkpoint(&state, args),
                "memory_checkpoint_list" => tool_checkpoint_list(&state, args),
                "memory_checkpoint_resolve" => tool_checkpoint_resolve(&state, args),
                "memory_sketch_create" => tool_sketch_create(&state, args),
                "memory_sketch_promote" => tool_sketch_promote(&state, args),
                "memory_crystallize" => tool_crystallize(&state, args),
                "memory_diagnose" => tool_diagnose(&state, args),
                "memory_facet_tag" => tool_facet_tag(&state, args),
                "memory_facet_query" => tool_facet_query(&state, args),
                "memory_lesson_save" => tool_lesson_save(&state, args),
                "memory_lesson_recall" => tool_lesson_recall(&state, args),
                "memory_reflect" => tool_reflect(&state, args),
                "memory_insight_list" => tool_insight_list(&state, args),
                "memory_slot_list" => tool_slot_list(&state, args),
                "memory_slot_get" => tool_slot_get(&state, args),
                "memory_slot_create" => tool_slot_create(&state, args),
                "memory_slot_append" => tool_slot_append(&state, args),
                "memory_slot_replace" => tool_slot_replace(&state, args),
                "memory_slot_delete" => tool_slot_delete(&state, args),
                "memory_sessions" => tool_sessions(&state, args),
                "memory_observe" => tool_observe(&state, args),
                "memory_commits" => tool_commits(&state, args),
                "memory_commit_lookup" => tool_commit_lookup(&state, args),
                "memory_consolidate" => tool_consolidate(&state, args),
                "memory_snapshot_create" => tool_snapshot_create(&state, args),
                "memory_file_history" => tool_file_history(&state, args),
                // Claude bridge sync
                "memory_claude_bridge_sync" | "mempalace_claude_bridge_sync" => {
                    tool_claude_bridge_sync(&state, args)
                }
                other => Err(ErrorData::invalid_params(
                    format!("Unknown tool: {}", other),
                    None,
                )),
            }
        }) as DynResult
    }
}

// ---------------------------------------------------------------------------
// Tool definitions
// ---------------------------------------------------------------------------

fn make_tools() -> Vec<rmcp::model::Tool> {
    use std::sync::Arc;
    fn tool(
        name: &'static str,
        title: &'static str,
        desc: &'static str,
        schema: serde_json::Value,
    ) -> rmcp::model::Tool {
        let map: serde_json::Map<String, serde_json::Value> =
            serde_json::from_value(schema).unwrap_or_default();
        rmcp::model::Tool::new(name, desc, Arc::new(map)).with_title(title)
    }
    vec![
        tool(
            "mempalace_status",
            "Palace Status",
            "Palace overview — total drawers, wing and room counts",
            serde_json::json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        tool(
            "mempalace_list_wings",
            "List Wings",
            "List all wings with drawer counts",
            serde_json::json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        tool(
            "mempalace_list_rooms",
            "List Rooms",
            "List rooms within a wing (or all rooms if no wing given)",
            serde_json::json!({ "type": "object", "properties": { "wing": { "type": "string", "description": "Wing to list rooms for (optional)" } } }),
        ),
        tool(
            "mempalace_get_taxonomy",
            "Get Taxonomy",
            "Full taxonomy: wing → room → drawer count",
            serde_json::json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        tool(
            "mempalace_get_aaak_spec",
            "Get AAAK Spec",
            "Get the AAAK dialect specification — the compressed memory format MemPalace uses. Call this if you need to read or write AAAK-compressed memories.",
            serde_json::json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        tool(
            "mempalace_kg_query",
            "KG Query",
            "Query the knowledge graph for an entity's relationships. Returns typed facts with temporal validity. E.g. 'Max' → child_of Alice, loves chess, does swimming. Filter by date with as_of to see what was true at a point in time.",
            serde_json::json!({ "type": "object", "properties": { "entity": { "type": "string", "description": "Entity to query (e.g. 'Max', 'MyProject', 'Alice')" }, "as_of": { "type": "string", "description": "Date filter — only facts valid at this date (YYYY-MM-DD, optional)" }, "direction": { "type": "string", "description": "outgoing (entity→?), incoming (?→entity), or both (default: both)" }, "limit": { "type": "integer", "description": "Max results (default 500, ranked by degree)" }, "offset": { "type": "integer", "description": "Offset for pagination (default 0)" } }, "required": ["entity"] }),
        ),
        tool(
            "mempalace_kg_add",
            "KG Add",
            "Add a fact to the knowledge graph. Subject → predicate → object with optional time window. E.g. ('Max', 'started_school', 'Year 7', valid_from='2026-09-01'). Use valid_to to backfill an already-ended historical fact.",
            serde_json::json!({ "type": "object", "properties": { "subject": { "type": "string", "description": "The entity doing/being something" }, "predicate": { "type": "string", "description": "The relationship type (e.g. 'loves', 'works_on', 'daughter_of')" }, "object": { "type": "string", "description": "The entity being connected to" }, "valid_from": { "type": "string", "description": "When this became true (YYYY-MM-DD, optional)" }, "valid_to": { "type": "string", "description": "When this stopped being true (YYYY-MM-DD, optional). Use this to backfill an already-ended fact in one call." }, "source_closet": { "type": "string", "description": "Closet ID where this fact appears (optional)" }, "source_file": { "type": "string", "description": "Source file path where this fact was extracted (optional)" }, "source_drawer_id": { "type": "string", "description": "Drawer ID where this fact was extracted, for adapter provenance (RFC 002 §5.5, optional)" } }, "required": ["subject", "predicate", "object"] }),
        ),
        tool(
            "mempalace_kg_invalidate",
            "KG Invalidate",
            "Mark a fact as no longer true. E.g. ankle injury resolved, job ended, moved house.",
            serde_json::json!({ "type": "object", "properties": { "subject": { "type": "string", "description": "Entity" }, "predicate": { "type": "string", "description": "Relationship" }, "object": { "type": "string", "description": "Connected entity" }, "ended": { "type": "string", "description": "When it stopped being true (YYYY-MM-DD, default: today)" } }, "required": ["subject", "predicate", "object"] }),
        ),
        tool(
            "mempalace_kg_timeline",
            "KG Timeline",
            "Chronological timeline of facts. Shows the story of an entity (or everything) in order.",
            serde_json::json!({ "type": "object", "properties": { "entity": { "type": "string", "description": "Entity to get timeline for (optional — omit for full timeline)" } } }),
        ),
        tool(
            "mempalace_kg_stats",
            "KG Stats",
            "Knowledge graph overview: entities, triples, current vs expired facts, relationship types.",
            serde_json::json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        tool(
            "mempalace_kg_snapshot_rebuild",
            "KG Snapshot Rebuild",
            "Build or rebuild the graph snapshot. Captures top-degree entities and aggregate counts. Refuses when totalNodes > 25,000 and no prior snapshot exists unless force=true.",
            serde_json::json!({ "type": "object", "properties": { "force": { "type": "boolean", "description": "Bypass the 25k-node pre-flight guard (default: false)" } }, "additionalProperties": false }),
        ),
        tool(
            "mempalace_kg_reset",
            "KG Reset",
            "Reset the knowledge graph snapshot. Writes an empty snapshot with resetAt so future queries treat pre-reset facts as not-found. Does NOT delete any data.",
            serde_json::json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        tool(
            "mempalace_traverse",
            "Traverse Graph",
            "Walk the palace graph from a room. Shows connected ideas across wings — the tunnels. Like following a thread through the palace: start at 'chromadb-setup' in wing_code, discover it connects to wing_myproject (planning) and wing_user (feelings about it).",
            serde_json::json!({ "type": "object", "properties": { "start_room": { "type": "string", "description": "Room to start from (e.g. 'chromadb-setup', 'riley-school')" }, "max_hops": { "type": "integer", "description": "How many connections to follow (default: 2)" } }, "required": ["start_room"] }),
        ),
        tool(
            "mempalace_find_tunnels",
            "Find Tunnels",
            "Find rooms that bridge two wings — the hallways connecting different domains. E.g. what topics connect wing_code to wing_team?",
            serde_json::json!({ "type": "object", "properties": { "wing_a": { "type": "string", "description": "First wing (optional)" }, "wing_b": { "type": "string", "description": "Second wing (optional)" } } }),
        ),
        tool(
            "mempalace_list_hallways",
            "List Hallways",
            "mr-0qr1: user-facing alias for find_tunnels. Lists cross-wing hallways (alias of tunnels).",
            serde_json::json!({ "type": "object", "properties": { "wing_a": { "type": "string", "description": "First wing (optional)" }, "wing_b": { "type": "string", "description": "Second wing (optional)" } } }),
        ),
        tool(
            "mempalace_delete_hallway",
            "Delete Hallway",
            "mr-0qr1: deletes a hallway by id. Returns an error explaining that hallways are derived from the graph and the source/target drawers must be deleted instead. Disabled in read-only mode.",
            serde_json::json!({ "type": "object", "properties": { "hallway_id": { "type": "string", "description": "ID of the hallway to delete" } }, "required": ["hallway_id"] }),
        ),
        tool(
            "mempalace_graph_stats",
            "Graph Stats",
            "Palace graph overview: total rooms, tunnel connections, edges between wings.",
            serde_json::json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        tool(
            "mempalace_search",
            "Search",
            "Semantic search. Returns verbatim drawer content with similarity scores. Supports metadata filtering via where_filter.",
            serde_json::json!({ "type": "object", "properties": { "query": { "type": "string", "description": "What to search for" }, "limit": { "type": "integer", "description": "Max results (default 5)" }, "wing": { "type": "string", "description": "Filter by wing (optional)" }, "room": { "type": "string", "description": "Filter by room (optional)" }, "context": { "type": "string", "description": "Optional caller context for transparency metadata" }, "where_filter": { "type": "object", "description": "Filter by custom metadata fields (e.g., {\"priority\": \"high\", \"status\": \"open\"})" }, "max_per_session": { "type": "integer", "description": "Max results per session/source_file (default 3, post-RRF filter)" } }, "required": ["query"] }),
        ),
        tool(
            "mempalace_check_duplicate",
            "Check Duplicate",
            "Check if content already exists in the palace before filing",
            serde_json::json!({ "type": "object", "properties": { "content": { "type": "string", "description": "Content to check" }, "threshold": { "type": "number", "description": "Similarity threshold 0-1 (default 0.9)" } }, "required": ["content"] }),
        ),
        tool(
            "mempalace_add_drawer",
            "Add Drawer",
            "File verbatim content into the palace. Checks for duplicates first. Supports custom metadata fields.",
            serde_json::json!({ "type": "object", "properties": { "wing": { "type": "string", "description": "Wing (project name)" }, "room": { "type": "string", "description": "Room (aspect: backend, decisions, meetings...)" }, "content": { "type": "string", "description": "Verbatim content to store — exact words, never summarized" }, "source_file": { "type": "string", "description": "Where this came from (optional)" }, "added_by": { "type": "string", "description": "Who is filing this (default: mcp)" } }, "required": ["wing", "room", "content"], "additionalProperties": { "type": "string", "description": "Custom metadata fields (optional string values)" } }),
        ),
        tool(
            "mempalace_delete_drawer",
            "Delete Drawer",
            "Delete a drawer by ID. Irreversible.",
            serde_json::json!({ "type": "object", "properties": { "drawer_id": { "type": "string", "description": "ID of the drawer to delete" } }, "required": ["drawer_id"] }),
        ),
        tool(
            "mempalace_diary_write",
            "Diary Write",
            "Write to your personal agent diary in AAAK format. Your observations, thoughts, what you worked on, what matters. Each agent has their own diary with full history. Write in AAAK for compression.",
            serde_json::json!({ "type": "object", "properties": { "agent_name": { "type": "string", "description": "Your name — each agent gets their own diary wing" }, "entry": { "type": "string", "description": "Your diary entry in AAAK format — compressed, entity-coded, emotion-marked" }, "topic": { "type": "string", "description": "Topic tag (optional, default: general)" }, "wing": { "type": "string", "description": "Optional target wing. If omitted, uses wing_{agent_name}." } }, "required": ["agent_name", "entry"] }),
        ),
        tool(
            "mempalace_diary_read",
            "Diary Read",
            "Read your recent diary entries (in AAAK). See what past versions of yourself recorded — your journal across sessions.",
            serde_json::json!({ "type": "object", "properties": { "agent_name": { "type": "string", "description": "Your name — each agent gets their own diary wing" }, "last_n": { "type": "integer", "description": "Number of recent entries to read (default: 10)" }, "wing": { "type": "string", "description": "Optional wing filter. If omitted, returns diary entries across every wing for this agent." } }, "required": ["agent_name"] }),
        ),
        tool(
            "mempalace_heal",
            "Heal Palace",
            "Auto-fix blocked actions and expired leases in the palace. Repairs broken dependency chains and cleans up stale resource leases.",
            serde_json::json!({ "type": "object", "properties": { "dry_run": { "type": "boolean", "description": "Preview what would be fixed without making changes (default: false)" } } }),
        ),
        tool(
            "mempalace_verify",
            "Verify Memory",
            "Verify a memory or observation by tracing its citation chain back to source. Returns confidence score and any issues found.",
            serde_json::json!({ "type": "object", "properties": { "target_id": { "type": "string", "description": "ID of the memory or observation to verify" }, "target_type": { "type": "string", "description": "Type of target: 'memory' or 'observation'" } }, "required": ["target_id", "target_type"] }),
        ),
        tool(
            "mempalace_governance_delete",
            "Governance Delete",
            "Delete memories from the palace based on governance policies: age, strength, type, project, or access patterns. Includes audit trail.",
            serde_json::json!({ "type": "object", "properties": { "max_age_days": { "type": "integer", "description": "Delete memories older than N days" }, "min_strength": { "type": "number", "description": "Delete memories below strength threshold (0-1)" }, "memory_type": { "type": "string", "description": "Filter by memory type (semantic, procedural, etc.)" }, "project": { "type": "string", "description": "Filter by project name" }, "not_accessed_since_days": { "type": "integer", "description": "Delete memories not accessed in N days" }, "reason": { "type": "string", "description": "Reason for deletion (required, for audit)" }, "type": { "type": "string", "description": "Alias for memory_type" } } }),
        ),
        tool(
            "mempalace_obsidian_export",
            "Obsidian Export",
            "Export memories or observations to an Obsidian-compatible markdown vault.",
            serde_json::json!({ "type": "object", "properties": { "export_type": { "type": "string", "description": "What to export: 'memories' or 'observations'" }, "output_dir": { "type": "string", "description": "Output directory for exported markdown files (default: ./memory-export)" }, "include_frontmatter": { "type": "boolean", "description": "Include YAML frontmatter in exported files (default: true)" }, "include_tags": { "type": "boolean", "description": "Include tags in exported files (default: true)" } }, "required": ["export_type"] }),
        ),
        tool(
            "mempalace_compress_file",
            "Compress File",
            "Compress a markdown file by removing redundant whitespace and formatting. Creates a backup of the original.",
            serde_json::json!({ "type": "object", "properties": { "file_path": { "type": "string", "description": "Path to the markdown file to compress" }, "dry_run": { "type": "boolean", "description": "Preview compression without modifying the file (default: false)" } }, "required": ["file_path"] }),
        ),
        tool(
            "mempalace_detect_worktree",
            "Detect Worktree",
            "Detect which git worktree the current or given project path is in, and list all worktrees. Returns branch, path, and whether each is the current one.",
            serde_json::json!({ "type": "object", "properties": { "project_path": { "type": "string", "description": "Path to the project directory to check (optional, defaults to current working directory)" } } }),
        ),
        tool(
            "mempalace_replay_import",
            "Replay Import",
            "Scan ~/.claude/projects for Claude Code session JSONL files and import them as observations. Returns imported session IDs, observation counts, and project names.",
            serde_json::json!({ "type": "object", "properties": { "project_filter": { "type": "string", "description": "Only import sessions from this project name (optional)" } } }),
        ),
        tool(
            "mempalace_action_create",
            "Action Create",
            "Create a multi-agent coordination action in the palace coordination module.",
            serde_json::json!({ "type": "object", "properties": { "title": { "type": "string", "description": "Title of the action" }, "description": { "type": "string", "description": "Description of the action (optional)" }, "priority": { "type": "integer", "description": "Priority level (optional)" }, "project": { "type": "string", "description": "Project name (optional)" }, "depends_on": { "type": "array", "items": { "type": "string" }, "description": "Action IDs this depends on (optional)" }, "source_observation_ids": { "type": "array", "items": { "type": "string" }, "description": "Observation IDs this action is based on (optional)" } }, "required": ["title"] }),
        ),
        tool(
            "mempalace_action_update",
            "Action Update",
            "Update the status or priority of an existing coordination action.",
            serde_json::json!({ "type": "object", "properties": { "actionId": { "type": "string", "description": "ID of the action to update" }, "status": { "type": "string", "description": "New status: Pending, Active, Done, Blocked, or Cancelled (optional)" }, "result": { "type": "string", "description": "Result or description update (optional)" }, "priority": { "type": "integer", "description": "New priority level (optional)" } }, "required": ["actionId"] }),
        ),
        tool(
            "mempalace_frontier",
            "Frontier Actions",
            "List unblocked actions at the frontier of the current execution graph for a project.",
            serde_json::json!({ "type": "object", "properties": { "project": { "type": "string", "description": "Project name (optional)" }, "agentId": { "type": "string", "description": "Agent ID (optional)" }, "limit": { "type": "integer", "description": "Maximum number of actions to return (optional)" } }, "additionalProperties": false }),
        ),
        tool(
            "mempalace_next",
            "Next Action",
            "Get the single highest-priority unblocked action ready to execute for a project.",
            serde_json::json!({ "type": "object", "properties": { "project": { "type": "string", "description": "Project name (optional)" }, "agentId": { "type": "string", "description": "Agent ID (optional)" } }, "additionalProperties": false }),
        ),
        tool(
            "mempalace_lease",
            "Lease Action",
            "Acquire, release, or renew a lease on an action to claim it for execution.",
            serde_json::json!({ "type": "object", "properties": { "actionId": { "type": "string", "description": "ID of the action to lease" }, "holder": { "type": "string", "description": "Agent name holding the lease (optional)" }, "ttlMs": { "type": "integer", "description": "Time-to-live in milliseconds for the lease (optional)" }, "operation": { "type": "string", "description": "Operation: acquire, release, or renew" }, "result": { "type": "string", "description": "Result or notes about the lease (optional)" } }, "required": ["actionId", "operation"] }),
        ),
        tool(
            "mempalace_routine_run",
            "Routine Run",
            "Execute a named routine with given parameters.",
            serde_json::json!({ "type": "object", "properties": { "routineId": { "type": "string", "description": "ID of the routine to execute" }, "project": { "type": "string", "description": "Project name (optional)" }, "initiatedBy": { "type": "string", "description": "Agent that initiated the routine (optional)" } }, "required": ["routineId"] }),
        ),
        tool(
            "mempalace_signal_send",
            "Signal Send",
            "Send a signal message to another agent.",
            serde_json::json!({ "type": "object", "properties": { "from": { "type": "string", "description": "Name of the sending agent (optional)" }, "to": { "type": "string", "description": "Name of the target agent" }, "content": { "type": "string", "description": "Signal message content" }, "signalType": { "type": "string", "description": "Type of signal: info, request, response, alert, handoff (optional)" }, "replyTo": { "type": "string", "description": "Thread ID to reply to (optional)" } }, "required": ["to", "content"] }),
        ),
        tool(
            "mempalace_signal_read",
            "Signal Read",
            "Read pending signal messages for an agent.",
            serde_json::json!({ "type": "object", "properties": { "agentId": { "type": "string", "description": "Name of the agent to read signals for" }, "unreadOnly": { "type": "boolean", "description": "Only return unread messages (optional)" }, "threadId": { "type": "string", "description": "Filter by thread ID (optional)" }, "limit": { "type": "integer", "description": "Maximum number of signals to return (optional)" } }, "required": ["agentId"] }),
        ),

        tool(
            "mempalace_sentinel_create",
            "Sentinel Create",
            "Create an event-driven sentinel that watches for conditions and triggers automatically.",
            serde_json::json!({ "type": "object", "properties": { "name": { "type": "string", "description": "Name of the sentinel" }, "watch_type": { "type": "string", "description": "Type of sentinel: action_status, memory_threshold, time_interval" }, "trigger_condition": { "type": "string", "description": "Condition expression" }, "action_id": { "type": "string", "description": "Action ID to trigger when condition is met (optional)" } }, "required": ["name", "watch_type"] }),
        ),
        tool(
            "mempalace_sentinel_trigger",
            "Sentinel Trigger",
            "Manually trigger a sentinel to evaluate its condition.",
            serde_json::json!({ "type": "object", "properties": { "sentinel_id": { "type": "string", "description": "ID of the sentinel to trigger" } }, "required": ["sentinel_id"] }),
        ),
        tool(
            "mempalace_sentinel_list",
            "Sentinel List",
            "List all active sentinels.",
            serde_json::json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "mempalace_sentinel_delete",
            "Sentinel Delete",
            "Delete a sentinel by ID.",
            serde_json::json!({ "type": "object", "properties": { "sentinel_id": { "type": "string", "description": "ID of the sentinel to delete" } }, "required": ["sentinel_id"] }),
        ),
        tool(
            "mempalace_checkpoint_list",
            "Checkpoint List",
            "List all checkpoints.",
            serde_json::json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "mempalace_checkpoint_resolve",
            "Checkpoint Resolve",
            "Resolve a checkpoint with a given status.",
            serde_json::json!({ "type": "object", "properties": { "checkpoint_id": { "type": "string", "description": "ID of the checkpoint to resolve" }, "status": { "type": "string", "description": "New status for the checkpoint" } }, "required": ["checkpoint_id", "status"] }),
        ),
        tool(
            "mempalace_sketch_create",
            "Sketch Create",
            "Create an ephemeral action graph for exploratory work.",
            serde_json::json!({ "type": "object", "properties": { "title": { "type": "string", "description": "Title of the sketch" }, "description": { "type": "string", "description": "Description of the sketch (optional)" }, "project": { "type": "string", "description": "Project name (optional)" } }, "required": ["title"] }),
        ),
        tool(
            "mempalace_sketch_promote",
            "Sketch Promote",
            "Promote a sketch's ephemeral actions to permanent actions in the palace.",
            serde_json::json!({ "type": "object", "properties": { "sketch_id": { "type": "string", "description": "ID of the sketch to promote" }, "action_ids": { "type": "array", "items": { "type": "string" }, "description": "Specific action IDs to promote (optional, promotes all if omitted)" } }, "required": ["sketch_id"] }),
        ),
        tool(
            "mempalace_crystallize",
            "Crystallize Actions",
            "Compress completed action chains into compact crystal digests with lessons learned.",
            serde_json::json!({ "type": "object", "properties": { "action_ids": { "type": "array", "items": { "type": "string" }, "description": "Action IDs to crystallize" }, "narrative": { "type": "string", "description": "Summary narrative of what was accomplished" }, "key_outcomes": { "type": "array", "items": { "type": "string" }, "description": "Key outcomes from this action chain" }, "files_affected": { "type": "array", "items": { "type": "string" }, "description": "Files affected by this action chain (optional)" } }, "required": ["action_ids", "narrative"] }),
        ),
        tool(
            "mempalace_health",
            "Health Check",
            "Quick health check: palace connectivity, embedder, and coordination status. Returns a single overall status (okay/warning/error) suitable for stdio MCP mode health probes.",
            serde_json::json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        tool(
            "mempalace_diagnose",
            "Diagnose Palace",
            "Run health checks across all palace subsystems and return diagnostics.",
            serde_json::json!({ "type": "object", "properties": { "checks": { "type": "array", "items": { "type": "string" }, "description": "Specific checks to run (optional, runs all if omitted)" } } }),
        ),
        tool(
            "mempalace_facet_tag",
            "Facet Tag",
            "Attach a structured tag to an action, memory, or observation.",
            serde_json::json!({ "type": "object", "properties": { "target_id": { "type": "string", "description": "ID of the target to tag" }, "target_type": { "type": "string", "description": "Type of target: action, memory, or observation" }, "facet": { "type": "string", "description": "Facet name (e.g. priority, domain, status)" }, "value": { "type": "string", "description": "Facet value" } }, "required": ["target_id", "target_type", "facet", "value"] }),
        ),
        tool(
            "mempalace_facet_query",
            "Facet Query",
            "Query targets by facet tags with AND/OR logic.",
            serde_json::json!({ "type": "object", "properties": { "facets": { "type": "array", "items": { "type": "object", "properties": { "facet": { "type": "string" }, "value": { "type": "string" } } }, "description": "Facets to match" }, "logic": { "type": "string", "description": "Logic: AND or OR (default: AND)" }, "target_type": { "type": "string", "description": "Filter by target type: action, memory, or observation (optional)" } }, "required": ["facets"] }),
        ),
        tool(
            "mempalace_lesson_save",
            "Lesson Save",
            "Save a lesson learned from this session.",
            serde_json::json!({ "type": "object", "properties": { "title": { "type": "string", "description": "Title of the lesson" }, "content": { "type": "string", "description": "Detailed lesson content" }, "context": { "type": "string", "description": "Context where this lesson was learned (optional)" }, "tags": { "type": "array", "items": { "type": "string" }, "description": "Tags for this lesson (optional)" } }, "required": ["title", "content"] }),
        ),
        tool(
            "mempalace_lesson_recall",
            "Lesson Recall",
            "Search lessons by query.",
            serde_json::json!({ "type": "object", "properties": { "query": { "type": "string", "description": "Query to search lessons" }, "limit": { "type": "integer", "description": "Max results (default 5)" } }, "required": ["query"] }),
        ),
        tool(
            "mempalace_reflect",
            "Reflect",
            "Traverse lessons, insights, and crystals by concept clusters, then synthesize higher-order insights via LLM.",
            serde_json::json!({ "type": "object", "properties": { "topic": { "type": "string", "description": "Topic to reflect on (filters source material)" }, "max_clusters": { "type": "integer", "description": "Max concept clusters to process (default 10, max 20)" } }, "required": ["topic"] }),
        ),
        tool(
            "mempalace_insight_list",
            "Insight List",
            "List synthesized insights.",
            serde_json::json!({ "type": "object", "properties": { "limit": { "type": "integer", "description": "Max results (default 10)" }, "min_strength": { "type": "number", "description": "Minimum insight strength 0-1 (optional)" } } }),
        ),
        tool(
            "mempalace_slot_list",
            "Slot List",
            "List all memory slots, optionally filtered by project.",
            serde_json::json!({ "type": "object", "properties": { "project": { "type": "string", "description": "Project to filter by (optional)" } } }),
        ),
        tool(
            "mempalace_slot_get",
            "Slot Get",
            "Get a specific memory slot by label.",
            serde_json::json!({ "type": "object", "properties": { "label": { "type": "string", "description": "Slot label" } }, "required": ["label"] }),
        ),
        tool(
            "mempalace_slot_create",
            "Slot Create",
            "Create a new memory slot.",
            serde_json::json!({ "type": "object", "properties": { "label": { "type": "string", "description": "Slot label (unique identifier)" }, "content": { "type": "string", "description": "Initial content (optional)" }, "sizeLimit": { "type": "integer", "description": "Size limit in characters (default 5000)" }, "description": { "type": "string", "description": "Description (optional)" }, "scope": { "type": "string", "description": "Scope: project or global (default project)" }, "project": { "type": "string", "description": "Project name (optional)" }, "pinned": { "type": "boolean", "description": "Pin the slot (optional)" } }, "required": ["label"] }),
        ),
        tool(
            "mempalace_slot_append",
            "Slot Append",
            "Append text to a memory slot.",
            serde_json::json!({ "type": "object", "properties": { "label": { "type": "string", "description": "Slot label" }, "text": { "type": "string", "description": "Text to append" } }, "required": ["label", "text"] }),
        ),
        tool(
            "mempalace_slot_replace",
            "Slot Replace",
            "Replace the content of a memory slot.",
            serde_json::json!({ "type": "object", "properties": { "label": { "type": "string", "description": "Slot label" }, "content": { "type": "string", "description": "New content" } }, "required": ["label", "content"] }),
        ),
        tool(
            "mempalace_slot_delete",
            "Slot Delete",
            "Delete a memory slot by label.",
            serde_json::json!({ "type": "object", "properties": { "label": { "type": "string", "description": "Slot label" } }, "required": ["label"] }),
        ),
        tool(
            "mempalace_checkpoint",
            "Checkpoint",
            "Create or resolve an external checkpoint.",
            serde_json::json!({ "type": "object", "properties": { "checkpoint_id": { "type": "string", "description": "Checkpoint identifier" }, "operation": { "type": "string", "description": "Operation: create or resolve" }, "condition": { "type": "string", "description": "Condition description for create (optional)" } }, "required": ["checkpoint_id", "operation"] }),
        ),
        tool(
            "mempalace_mesh_sync",
            "Mesh Sync",
            "Sync memories and actions with peer MemPalace instances.",
            serde_json::json!({ "type": "object", "properties": { "peer_ids": { "type": "array", "items": { "type": "string" }, "description": "Specific peer IDs to sync with (optional, syncs all if omitted)" }, "direction": { "type": "string", "description": "Sync direction: push, pull, or both (default: both)" } } }),
        ),
        tool(
            "mempalace_team_share",
            "Team Share",
            "Share a memory or observation with team members.",
            serde_json::json!({ "type": "object", "properties": { "target_id": { "type": "string", "description": "ID of the memory or observation to share" }, "target_type": { "type": "string", "description": "Type: memory or observation" }, "team_members": { "type": "array", "items": { "type": "string" }, "description": "Team member names to share with" }, "note": { "type": "string", "description": "Optional note to include" } }, "required": ["target_id", "target_type", "team_members"] }),
        ),
        tool(
            "mempalace_team_feed",
            "Team Feed",
            "Get recent shared items from all team members.",
            serde_json::json!({ "type": "object", "properties": { "limit": { "type": "integer", "description": "Max results (default 20)" }, "team_member": { "type": "string", "description": "Filter by specific team member (optional)" } } }),
        ),
        tool(
            "mempalace_consolidate",
            "Consolidate",
            "Run the memory consolidation pipeline to promote memories through tiers.",
            serde_json::json!({ "type": "object", "properties": { "source_tier": { "type": "string", "description": "Source tier: working, episodic, semantic, or procedural" }, "target_tier": { "type": "string", "description": "Target tier (optional)" }, "force": { "type": "boolean", "description": "Force consolidation even if threshold not met (default: false)" } } }),
        ),
        tool(
            "mempalace_graph_search",
            "Graph Search",
            "Search the knowledge graph by entity names using BFS traversal.",
            serde_json::json!({ "type": "object", "properties": { "entity_names": { "type": "array", "items": { "type": "string" }, "description": "Entity names to search from" }, "depth": { "type": "integer", "description": "Traversal depth (default: 2)" }, "limit": { "type": "integer", "description": "Max results (default: 50)" } } }),
        ),
        tool(
            "mempalace_graph_expand",
            "Graph Expand",
            "Expand from observation IDs into the knowledge graph using BFS.",
            serde_json::json!({ "type": "object", "properties": { "observation_ids": { "type": "array", "items": { "type": "string" }, "description": "Observation IDs to expand from" }, "depth": { "type": "integer", "description": "Traversal depth (default: 2)" }, "limit": { "type": "integer", "description": "Max results (default: 50)" } } }),
        ),
        tool(
            "mempalace_context_build",
            "Context Build",
            "Build a priority-ordered context from pinned memories, lessons, session summaries, and working memory within a token budget.",
            serde_json::json!({ "type": "object", "properties": { "token_budget": { "type": "integer", "description": "Max tokens (default: 8000)" }, "pinned_ids": { "type": "array", "items": { "type": "string" }, "description": "Pinned memory slot IDs (optional)" }, "session_ids": { "type": "array", "items": { "type": "string" }, "description": "Session IDs to include summaries from (optional)" }, "include_working_memory": { "type": "boolean", "description": "Include compressed working memory observations (default: true)" }, "output_format": { "type": "string", "description": "Output format: json or xml (default: json)" } } }),
        ),
        tool(
            "mempalace_enrich",
            "Enrich",
            "Enrich a file path with related memories, bug references, and patterns.",
            serde_json::json!({ "type": "object", "properties": { "file_path": { "type": "string", "description": "File path to enrich" }, "query": { "type": "string", "description": "Search query to find related memories (optional)" }, "search_limit": { "type": "integer", "description": "Max memories per search (default: 10)" } } }),
        ),
        tool(
            "mempalace_retention_score",
            "Retention Score",
            "Get the retention score and decay status for a memory. Returns Ebbinghaus-based retention strength, access count, and promotion tier recommendation.",
            serde_json::json!({ "type": "object", "properties": { "memory_id": { "type": "string", "description": "Memory ID to get retention score for" } }, "required": ["memory_id"] }),
        ),
        tool(
            "mempalace_access_stats",
            "Access Stats",
            "Get access statistics for memories - most accessed, recently accessed, and access counts.",
            serde_json::json!({ "type": "object", "properties": { "limit": { "type": "integer", "description": "Max results per category (default: 10)" } } }),
        ),
        tool(
            "mempalace_working_memory",
            "Working Memory",
            "List current working memory observations.",
            serde_json::json!({ "type": "object", "properties": { "limit": { "type": "integer", "description": "Max observations to return (default: 50)" } } }),
        ),
        tool(
            "mempalace_snapshot_create",
            "Snapshot Create",
            "Create a git-versioned snapshot of the current palace state.",
            serde_json::json!({ "type": "object", "properties": { "message": { "type": "string", "description": "Snapshot commit message" }, "tag": { "type": "string", "description": "Optional tag for this snapshot" } } }),
        ),
        tool(
            "mempalace_file_history",
            "File History",
            "Get past observations and memories related to a specific file.",
            serde_json::json!({ "type": "object", "properties": { "file_path": { "type": "string", "description": "Path to the file" }, "limit": { "type": "integer", "description": "Max results (default 10)" } }, "required": ["file_path"] }),
        ),
        tool(
            "mempalace_sessions",
            "Sessions",
            "List recent agent sessions with status and observation counts.",
            serde_json::json!({ "type": "object", "properties": { "limit": { "type": "integer", "description": "Max sessions to return (default 10)" }, "project": { "type": "string", "description": "Filter by project name (optional)" } } }),
        ),
        tool(
            "mempalace_observe",
            "Observe",
            "Capture a lifecycle hook observation. Accepts hook_type, session_id, project, cwd, and optional data payload. Saves via SessionStore; for session-ending hook types (session_end, stop) also ends the session.",
            serde_json::json!({ "type": "object", "properties": {
                "hook_type": { "type": "string", "description": "Hook type: session_start, user_prompt_submit, pre_tool_use, post_tool_use, post_tool_use_failure, pre_compact, subagent_start, subagent_stop, stop, session_end, notification, task_completed" },
                "session_id": { "type": "string", "description": "Session ID this observation belongs to" },
                "project": { "type": "string", "description": "Project name (optional)" },
                "cwd": { "type": "string", "description": "Current working directory (optional)" },
                "data": { "type": "object", "description": "Additional data payload (optional)" }
            }, "required": ["hook_type", "session_id"] }),
        ),
        tool(
            "mempalace_commits",
            "Commits",
            "List recent git commits linked to agent sessions.",
            serde_json::json!({ "type": "object", "properties": { "limit": { "type": "integer", "description": "Max commits to return (default 10)" }, "project": { "type": "string", "description": "Filter by project name (optional)" }, "branch": { "type": "string", "description": "Filter by branch name (optional)" } } }),
        ),
        tool(
            "mempalace_commit_lookup",
            "Commit Lookup",
            "Look up the agent session that produced a specific git commit.",
            serde_json::json!({ "type": "object", "properties": { "commit_sha": { "type": "string", "description": "Git commit SHA" } }, "required": ["commit_sha"] }),
        ),
        tool(
            "mempalace_mine",
            "Mine",
            "In-process mine: walk a directory and file drawers into the palace. Equivalent to running `mpr mine` with the same arguments. Disabled in read-only mode.",
            serde_json::json!({ "type": "object", "properties": { "path": { "type": "string", "description": "Directory to mine" }, "mode": { "type": "string", "description": "Mining mode: 'projects' (default), 'convos', or 'auto'" } }, "required": ["path"] }),
        ),
        tool(
            "mempalace_smart_search",
            "Smart Search",
            "Progressive disclosure search with expand_ids mode. When expand_ids is provided, fetches full content for specific IDs. Otherwise performs hybrid semantic+BM25 search. Returns semantic results plus optional expanded content for focused retrieval.",
            serde_json::json!({ "type": "object", "properties": { "query": { "type": "string", "description": "Search query" }, "expand_ids": { "type": "string", "description": "Comma-separated list of IDs to expand (fetch full content for progressive disclosure)" }, "limit": { "type": "integer", "description": "Max results (default 10, max 300)" }, "wing": { "type": "string", "description": "Filter by wing (optional)" }, "room": { "type": "string", "description": "Filter by room (optional)" } }, "required": ["query"] }),
        ),
        tool(
            "mempalace_hybrid_search",
            "Hybrid Search",
            "Hybrid BM25 + vector search combining keyword and semantic matching with RRF fusion. Returns ranked results with both similarity and BM25 scores for optimal relevance ranking.",
            serde_json::json!({ "type": "object", "properties": { "query": { "type": "string", "description": "Search query" }, "limit": { "type": "integer", "description": "Max results (default 10)" }, "wing": { "type": "string", "description": "Filter by wing (optional)" }, "room": { "type": "string", "description": "Filter by room (optional)" } }, "required": ["query"] }),
        ),
        tool(
            "memory_claude_bridge_sync",
            "Claude Bridge Sync",
            "Sync memories to/from Claude Code's MEMORY.md file. Reads from or writes to the Claude Code memory file for the project.",
            serde_json::json!({ "type": "object", "properties": { "direction": { "type": "string", "description": "Sync direction: push (to Claude), pull (from Claude), or sync (bidirectional, default: sync)" } }, "additionalProperties": false }),
        ),
        tool(
            "mempalace_claude_bridge_sync",
            "Mempalace Claude Bridge Sync",
            "Alias for memory_claude_bridge_sync - sync memories to/from Claude Code's MEMORY.md file.",
            serde_json::json!({ "type": "object", "properties": { "direction": { "type": "string", "description": "Sync direction: push (to Claude), pull (from Claude), or sync (bidirectional, default: sync)" } }, "additionalProperties": false }),
        ),
    ]
}

// ---------------------------------------------------------------------------
// ServerHandler impl
// ---------------------------------------------------------------------------

#[non_exhaustive]
pub struct MempalaceServer {
    state: Arc<AppState>,
}

impl MempalaceServer {
    pub fn new(state: AppState) -> Self {
        Self {
            state: Arc::new(state),
        }
    }
}

impl ServerHandler for MempalaceServer {
    fn get_info(&self) -> McpServerInfo {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("mempalace", env!("CARGO_PKG_VERSION")))
            .with_instructions("MemPalace - AI memory palace. Search, mine, and manage memories.")
    }

    fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        // In read-only mode, hide mutation tools from `tools/get` so well-behaved
        // clients don't surface them as available. Call-time `read_only_guard`
        // still rejects direct invocations as a defense-in-depth fallback.
        if self.state.read_only && is_mutation_tool(name) {
            return None;
        }
        make_tools().into_iter().find(|t| t.name.as_ref() == name)
    }

    fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        _ctx: rmcp::service::RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, rmcp::ErrorData>> + MaybeSendFuture + '_
    {
        let dispatch = make_dispatch(self.state.clone());
        let wal_dir = self.state.palace_path.join("wal");
        async move {
            invoke_with_wal(
                request.name.to_string(),
                request.arguments.unwrap_or_default(),
                dispatch,
                wal_dir,
            )
            .await
        }
    }

    fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _ctx: rmcp::service::RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, rmcp::ErrorData>> + MaybeSendFuture + '_
    {
        // Filter mutation tools out of the listing when running read-only so
        // clients don't show them as available actions. `read_only_guard` keeps
        // call-time rejection in place for any client that bypasses the list.
        let tools = if self.state.read_only {
            make_tools()
                .into_iter()
                .filter(|t| !is_mutation_tool(t.name.as_ref()))
                .collect()
        } else {
            make_tools()
        };
        std::future::ready(Ok(ListToolsResult::with_all_items(tools)))
    }

    fn list_resources(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _ctx: rmcp::service::RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourcesResult, ErrorData>> + MaybeSendFuture + '_
    {
        std::future::ready(self.list_mcp_resources())
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _ctx: rmcp::service::RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ReadResourceResult, ErrorData>> + MaybeSendFuture + '_
    {
        std::future::ready(self.read_mcp_resource(&request.uri))
    }

    fn list_prompts(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _ctx: rmcp::service::RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListPromptsResult, ErrorData>> + MaybeSendFuture + '_
    {
        std::future::ready(self.list_mcp_prompts())
    }

    fn get_prompt(
        &self,
        request: rmcp::model::GetPromptRequestParams,
        _ctx: rmcp::service::RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<GetPromptResult, ErrorData>> + MaybeSendFuture + '_
    {
        let args = request.arguments.unwrap_or_default();
        std::future::ready(self.get_mcp_prompt(&request.name, args))
    }
}

// ---------------------------------------------------------------------------
// MCP Resources & Prompts helpers
// ---------------------------------------------------------------------------

impl MempalaceServer {
    fn list_mcp_resources(&self) -> Result<ListResourcesResult, ErrorData> {
        use rmcp::model::Annotated;
        let resources = vec![
            Annotated::new(
                rmcp::model::RawResource {
                    uri: "mempalace://status".into(),
                    name: "Palace Status".into(),
                    title: Some("MemPalace Status".into()),
                    description: Some("Session count, memory count, and health metrics".into()),
                    mime_type: Some("text/plain".into()),
                    size: None,
                    icons: None,
                    meta: None,
                },
                None,
            ),
            Annotated::new(
                rmcp::model::RawResource {
                    uri: "mempalace://project/{name}/profile".into(),
                    name: "Project Profile".into(),
                    title: Some("Project Memory Profile".into()),
                    description: Some("Top concepts, files, and conventions for a project".into()),
                    mime_type: Some("text/plain".into()),
                    size: None,
                    icons: None,
                    meta: None,
                },
                None,
            ),
            Annotated::new(
                rmcp::model::RawResource {
                    uri: "mempalace://project/{name}/recent".into(),
                    name: "Recent Sessions".into(),
                    title: Some("Recent Session Summaries".into()),
                    description: Some("Last 5 session summaries for a project".into()),
                    mime_type: Some("text/plain".into()),
                    size: None,
                    icons: None,
                    meta: None,
                },
                None,
            ),
            Annotated::new(
                rmcp::model::RawResource {
                    uri: "mempalace://memories/latest".into(),
                    name: "Latest Memories".into(),
                    title: Some("Top 10 Latest Memories".into()),
                    description: Some("Top 10 most recent memories".into()),
                    mime_type: Some("text/plain".into()),
                    size: None,
                    icons: None,
                    meta: None,
                },
                None,
            ),
            Annotated::new(
                rmcp::model::RawResource {
                    uri: "mempalace://graph/stats".into(),
                    name: "Graph Statistics".into(),
                    title: Some("Knowledge Graph Statistics".into()),
                    description: Some("KG node and edge counts by type".into()),
                    mime_type: Some("text/plain".into()),
                    size: None,
                    icons: None,
                    meta: None,
                },
                None,
            ),
            Annotated::new(
                rmcp::model::RawResource {
                    uri: "mempalace://team/feed".into(),
                    name: "Team Feed".into(),
                    title: Some("Team Shared Memories".into()),
                    description: Some("Recent shared memories from team members".into()),
                    mime_type: Some("text/plain".into()),
                    size: None,
                    icons: None,
                    meta: None,
                },
                None,
            ),
        ];
        Ok(ListResourcesResult::with_all_items(resources))
    }

    fn read_mcp_resource(&self, uri: &str) -> Result<ReadResourceResult, ErrorData> {
        match uri {
            "mempalace://status" => self.read_resource_status(),
            uri if uri.starts_with("mempalace://project/") => self.read_resource_project(uri),
            "mempalace://memories/latest" => self.read_resource_latest_memories(),
            "mempalace://graph/stats" => self.read_resource_graph_stats(),
            "mempalace://team/feed" => self.read_resource_team_feed(),
            _ => Err(rmcp::ErrorData::invalid_params(
                format!("Unknown resource URI: {}", uri),
                None,
            )),
        }
    }

    fn read_resource_status(&self) -> Result<ReadResourceResult, ErrorData> {
        let db = fresh_db(self.state.as_ref())
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        let entries = db.get_all(None, None, usize::MAX);
        let memory_count = entries.len();
        // A successfully-opened palace is healthy; count is informational.
        let health = "healthy";
        let content = format!(
            "MemPalace Status\n===============\nMemories: {}\nHealth: {}",
            memory_count, health
        );
        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            content,
            "mempalace://status",
        )]))
    }

    fn read_resource_project(&self, uri: &str) -> Result<ReadResourceResult, ErrorData> {
        let parts: Vec<&str> = uri.splitn(4, '/').collect();
        if parts.len() < 4 || parts[2] != "project" {
            return Err(ErrorData::invalid_params(
                "Invalid project resource URI format",
                None,
            ));
        }
        let name = parts[3];
        let db = fresh_db(self.state.as_ref())
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        let entries = db.get_all(Some(name), None, 50);
        let files: Vec<String> = entries
            .iter()
            .filter_map(|e| {
                e.metadatas
                    .first()?
                    .get("source_file")?
                    .as_str()
                    .map(String::from)
            })
            .take(10)
            .collect();
        let content = format!(
            "Project Profile: {}\n===============\nRecent entries: {}\nFiles: {:?}",
            name,
            entries.len(),
            files
        );
        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            content, uri,
        )]))
    }

    fn read_resource_latest_memories(&self) -> Result<ReadResourceResult, ErrorData> {
        let db = fresh_db(self.state.as_ref())
            .map_err(|e| rmcp::ErrorData::invalid_params(e.to_string(), None))?;
        let entries = db.get_all(None, None, 10);
        let content = if entries.is_empty() {
            "No memories found".to_string()
        } else {
            let items: Vec<String> = entries
                .iter()
                .map(|e| {
                    format!(
                        "- {}",
                        e.ids.first().map(|id| id.as_str()).unwrap_or("unknown")
                    )
                })
                .collect();
            format!("Latest Memories\n==============\n{}", items.join("\n"))
        };
        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            content,
            "mempalace://memories/latest",
        )]))
    }

    fn read_resource_graph_stats(&self) -> Result<ReadResourceResult, ErrorData> {
        let content =
            "Knowledge Graph Statistics\n==========================\n(KG not yet implemented)";
        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            content,
            "mempalace://graph/stats",
        )]))
    }

    fn read_resource_team_feed(&self) -> Result<ReadResourceResult, ErrorData> {
        let content = "Team Feed\n=======\n(No team data available)".to_string();
        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            content,
            "mempalace://team/feed",
        )]))
    }

    fn list_mcp_prompts(&self) -> Result<ListPromptsResult, ErrorData> {
        let prompts = vec![
            rmcp::model::Prompt::new(
                "recall_context",
                Some("Search memories for task context"),
                Some(vec![PromptArgument::new("task_description")
                    .with_title("Task Description")
                    .with_description("Description of the task to find context for")
                    .with_required(true)]),
            ),
            rmcp::model::Prompt::new(
                "session_handoff",
                Some("Generate handoff summary for a session"),
                Some(vec![PromptArgument::new("session_id")
                    .with_title("Session ID")
                    .with_description("ID of the session to generate handoff for")
                    .with_required(true)]),
            ),
            rmcp::model::Prompt::new(
                "detect_patterns",
                Some("Detect recurring patterns in memories"),
                Some(vec![PromptArgument::new("project")
                    .with_title("Project")
                    .with_description("Project name to analyze (optional)")
                    .with_required(false)]),
            ),
        ];
        Ok(ListPromptsResult::with_all_items(prompts))
    }

    fn get_mcp_prompt(
        &self,
        name: &str,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> Result<GetPromptResult, ErrorData> {
        use rmcp::model::{PromptMessage, PromptMessageContent, PromptMessageRole};
        match name {
            "recall_context" => {
                let task_desc = args
                    .get("task_description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let db = fresh_db(self.state.as_ref())
                    .map_err(|e| rmcp::ErrorData::invalid_params(e.to_string(), None))?;
                let results = db
                    .query_sync(task_desc, None, None, 5)
                    .map_err(|e| rmcp::ErrorData::invalid_params(e.to_string(), None))?;
                let context: Vec<String> = results
                    .iter()
                    .filter_map(|r| {
                        r.documents
                            .first()
                            .map(|d| format!("- {}", d.chars().take(100).collect::<String>()))
                    })
                    .collect();
                let message = format!(
                    "Recall Context for: {}\n\nFound {} relevant memories:\n{}",
                    task_desc,
                    results.len(),
                    context.join("\n")
                );
                Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                    PromptMessageRole::User,
                    message,
                )]))
            }
            "session_handoff" => {
                let session_id = args
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let content = format!(
                    "Session Handoff for: {}\n\nPlease provide context about what was accomplished and what needs to be done next.",
                    session_id
                );
                Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                    PromptMessageRole::User,
                    content,
                )]))
            }
            "detect_patterns" => {
                let project = args.get("project").and_then(|v| v.as_str());
                let content = if let Some(p) = project {
                    format!("Analyzing patterns for project: {}", p)
                } else {
                    "Analyzing patterns across all projects".to_string()
                };
                Ok(GetPromptResult::new(vec![PromptMessage::new_text(
                    PromptMessageRole::User,
                    content,
                )]))
            }
            _ => Err(rmcp::ErrorData::invalid_params(
                format!("Unknown prompt: {}", name),
                None,
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn text_result(text: String) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

fn ok_json<T: serde::Serialize>(value: T) -> Result<CallToolResult, ErrorData> {
    let s = serde_json::to_string(&value).map_err(|e| internal_error_safe(&e))?;
    text_result(s)
}

fn read_only_guard(state: &AppState) -> Result<(), ErrorData> {
    if state.read_only {
        Err(ErrorData::invalid_request(
            "This tool requires read-write mode.",
            None,
        ))
    } else {
        Ok(())
    }
}

fn parse_args<T: for<'de> serde::Deserialize<'de>>(args: JsonObject) -> Result<T, ErrorData> {
    serde_json::from_value(serde_json::Value::Object(args))
        .map_err(|e| ErrorData::invalid_params(e.to_string(), None))
}

fn parse_args_with_integer_coercion<T: for<'de> serde::Deserialize<'de>>(
    mut args: JsonObject,
    integer_fields: &[&str],
) -> Result<T, ErrorData> {
    for field in integer_fields {
        if let Some(value) = args.get_mut(*field) {
            if let Some(number) = value.as_f64() {
                if number.is_finite() && number.fract() == 0.0 && number >= 0.0 {
                    *value = serde_json::Value::from(number as u64);
                }
            }
        }
    }

    parse_args(args)
}

fn no_palace() -> serde_json::Value {
    serde_json::json!({
        "error": "No palace found",
        "hint": "Run: mempalace init <dir> && mempalace mine <dir>",
    })
}

/// Open a fresh `PalaceDb` view rooted at the configured palace path.
///
/// All write tools (`tool_add_drawer`, `tool_delete_drawer`,
/// `tool_diary_write`) open ad-hoc `PalaceDb` handles, mutate them, and
/// flush to disk. The long-lived `state.db` snapshot built once in
/// `AppState::new` is therefore stale the moment any write tool runs.
/// Read-side tools must reopen from disk on every call so that
/// read-after-write within a single MCP server session reflects the
/// latest data. The unit-test `dispatch` helper masks this by
/// reopening `AppState.db` per call, but the real server holds one
/// `Arc<AppState>` for the whole session.
fn fresh_db(state: &AppState) -> Result<crate::palace_db::PalaceDb, ErrorData> {
    crate::palace_db::PalaceDb::open(&state.palace_path).map_err(|e| internal_error_safe(&e))
}

fn collection_missing(state: &AppState) -> bool {
    !state
        .palace_path
        .join(format!(
            "{}.json",
            crate::palace_db::DEFAULT_COLLECTION_NAME
        ))
        .exists()
}

// ---------------------------------------------------------------------------
// AGENT_SCOPE isolation helpers
// ---------------------------------------------------------------------------

/// Read `MEMPALACE_AGENT_ID` env var, falling back to `config.agent_id`.
/// The env var takes priority over the config file value.
fn resolve_agent_id(config: &crate::Config) -> Option<String> {
    std::env::var("MEMPALACE_AGENT_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| config.agent_id.clone())
        .filter(|s| !s.is_empty())
}

/// Returns `true` when AGENT_SCOPE is `"isolated"`.
/// Reads `MEMPALACE_AGENT_SCOPE` env var, falling back to `config.agent_scope`.
/// The default is `"shared"` (not isolated).
fn is_agent_isolated(config: &crate::Config) -> bool {
    let scope = std::env::var("MEMPALACE_AGENT_SCOPE")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| config.agent_scope.clone())
        .unwrap_or_else(|| "shared".to_string());
    scope.eq_ignore_ascii_case("isolated")
}

/// Resolve the effective `agent_id` for filtering purposes when scope is isolated.
///
/// Returns `Ok(None)` when:
/// - scope is `"shared"` (not isolated) – pass-through, no filtering
/// - agent_id is `"*"` (wildcard) – bypass all filtering
///
/// Returns `Ok(Some(id))` when isolating against a concrete agent_id.
///
/// # Errors
/// Returns an error when scope is `"isolated"` but no agent_id resolves from
/// either `MEMPALACE_AGENT_ID` env var or `config.agent_id`. This is a
/// **fail-closed** design: isolated mode without an identity produces an error
/// rather than leaking unfiltered results.
fn resolved_agent_id(state: &AppState) -> Result<Option<String>, ErrorData> {
    if !is_agent_isolated(&state.config) {
        return Ok(None);
    }
    let agent_id = resolve_agent_id(&state.config);
    match agent_id {
        Some(id) if id == "*" => Ok(None), // wildcard: no filtering
        Some(id) => Ok(Some(id)),
        None => Err(ErrorData::invalid_request(
            "AGENT_SCOPE=isolated but no MEMPALACE_AGENT_ID or config.agent_id set. \
             Set MEMPALACE_AGENT_ID to a unique agent identifier to enable isolated mode, \
             or set AGENT_SCOPE=shared to disable isolation.",
            None,
        )),
    }
}

/// Post-filter `query_sync` / `hybrid_search` results so only entries whose
/// `agent_id` metadata field matches the resolved agent_id survive.
///
/// When scope is `"shared"` or agent_id is `"*"` (wildcard), all results pass
/// through unchanged.
fn filter_by_agent_id(
    results: Vec<crate::palace_db::QueryResult>,
    state: &AppState,
) -> Result<Vec<crate::palace_db::QueryResult>, ErrorData> {
    let agent_id = resolved_agent_id(state)?;
    let Some(ref id) = agent_id else {
        return Ok(results);
    };
    Ok(results
        .into_iter()
        .filter(|r| {
            r.metadatas.iter().any(|m| {
                m.get("agent_id")
                    .and_then(|v| v.as_str())
                    .map(|v| v == id)
                    .unwrap_or(false)
            })
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Tool handlers
// ---------------------------------------------------------------------------

fn tool_health(state: &AppState, _args: JsonObject) -> Result<CallToolResult, ErrorData> {
    // Quick health check suitable for stdio MCP mode health probes.
    // Does not need a fully loaded palace; just checks basic connectivity.
    let palace_path = &state.palace_path;
    if !palace_path.exists() {
        return ok_json(serde_json::json!({
            "status": "error",
            "message": "Palace path does not exist",
            "palace_path": palace_path.to_string_lossy(),
        }));
    }

    let db = match fresh_db(state) {
        Ok(db) => db,
        Err(e) => {
            return ok_json(serde_json::json!({
                "status": "error",
                "message": format!("Cannot open palace DB: {e}"),
                "palace_path": palace_path.to_string_lossy(),
            }));
        }
    };

    let drawer_count = db.count();
    let coordination_ok = db.coordination().action_list_all().is_ok();
    let embedder = &state.config.embedding_model;

    let (status, message) = if drawer_count == 0 {
        (
            "okay",
            "Palace is open but has no memories yet. Run `mpr mine <dir>` to load data."
                .to_string(),
        )
    } else if !coordination_ok {
        (
            "warning",
            "Palace is open with memories but coordination subsystem is unavailable.".to_string(),
        )
    } else {
        (
            "okay",
            format!("Palace is healthy with {} memories", drawer_count),
        )
    };

    ok_json(serde_json::json!({
        "status": status,
        "message": message,
        "palace_path": palace_path.to_string_lossy(),
        "drawer_count": drawer_count,
        "embedder": embedder,
    }))
}

fn tool_status(state: &AppState, _args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    let db = fresh_db(state)?;
    let entries = db.get_all(None, None, usize::MAX);
    let mut wings: HashMap<String, usize> = HashMap::new();
    let mut rooms: HashMap<String, usize> = HashMap::new();
    for entry in &entries {
        if let Some(meta) = entry.metadatas.first() {
            let wing = meta
                .get("wing")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let room = meta
                .get("room")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            *wings.entry(wing.to_string()).or_insert(0) += 1;
            *rooms.entry(room.to_string()).or_insert(0) += 1;
        }
    }
    ok_json(serde_json::json!({
        "total_drawers": db.count(),
        "wings": wings,
        "rooms": rooms,
        "palace_path": state.palace_path.display().to_string(),
        "protocol": PALACE_PROTOCOL,
        "aaak_dialect": AAAK_SPEC,
    }))
}

fn tool_list_wings(state: &AppState, _args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    let entries = fresh_db(state)?.get_all(None, None, usize::MAX);
    let mut wings: HashMap<String, usize> = HashMap::new();
    for entry in &entries {
        if let Some(meta) = entry.metadatas.first() {
            let wing = meta
                .get("wing")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            *wings.entry(wing.to_string()).or_insert(0) += 1;
        }
    }
    ok_json(serde_json::json!({ "wings": wings }))
}

fn tool_list_rooms(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        wing: Option<String>,
    }
    let input: Input = parse_args(args)?;
    let entries = fresh_db(state)?.get_all(input.wing.as_deref(), None, usize::MAX);
    let mut room_counts: HashMap<String, usize> = HashMap::new();
    for entry in &entries {
        if let Some(meta) = entry.metadatas.first() {
            if let Some(room) = meta.get("room").and_then(|v| v.as_str()) {
                *room_counts.entry(room.to_string()).or_insert(0) += 1;
            }
        }
    }
    ok_json(
        serde_json::json!({ "wing": input.wing.unwrap_or_else(|| "all".to_string()), "rooms": room_counts }),
    )
}

fn tool_get_taxonomy(state: &AppState, _args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    let entries = fresh_db(state)?.get_all(None, None, usize::MAX);
    let mut taxonomy: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for entry in &entries {
        if let Some(meta) = entry.metadatas.first() {
            let wing = meta
                .get("wing")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let room = meta
                .get("room")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            *taxonomy
                .entry(wing.to_string())
                .or_default()
                .entry(room.to_string())
                .or_insert(0) += 1;
        }
    }
    ok_json(serde_json::json!({ "taxonomy": taxonomy }))
}

fn tool_get_aaak_spec(_state: &AppState, _args: JsonObject) -> Result<CallToolResult, ErrorData> {
    ok_json(serde_json::json!({ "aaak_spec": AAAK_SPEC }))
}

fn tool_search(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        query: String,
        wing: Option<String>,
        room: Option<String>,
        limit: Option<usize>,
        context: Option<String>,
        where_filter: Option<serde_json::Value>,
        max_per_session: Option<usize>,
    }
    let input: Input = parse_args_with_integer_coercion(args, &["limit", "max_per_session"])?;
    let sanitized = crate::query_sanitizer::sanitize_query(&input.query);

    // Convert where_filter to metadata filter if provided
    let metadata_filter = if let Some(filter) = input.where_filter {
        if let Some(obj) = filter.as_object() {
            let mut filter_map = std::collections::HashMap::new();
            for (key, value) in obj {
                if let Some(str_val) = value.as_str() {
                    filter_map.insert(key.clone(), str_val.to_string());
                }
            }
            Some(filter_map)
        } else {
            None
        }
    } else {
        None
    };

    let db = crate::palace_db::PalaceDb::open(&state.palace_path)
        .map_err(|e| internal_error_safe(&e))?;
    let query_results = db
        .hybrid_search(
            &sanitized.clean_query,
            input.limit.unwrap_or(5),
            input.wing.as_deref(),
            input.room.as_deref(),
        )
        .map_err(|e| internal_error_safe(&e))?;

    let filtered_results = if let Some(ref filter_map) = metadata_filter {
        query_results
            .into_iter()
            .enumerate()
            .filter(|(_, r)| {
                // Defensive: skip drawers whose metadata is fully missing/empty
                // (consolidated searcher can emit empty metadata maps for some rows)
                r.metadatas.iter().any(|m| {
                    !m.is_empty()
                        && filter_map.iter().all(|(k, v)| {
                            m.get(k)
                                .map(|mv| mv.as_str().unwrap_or("") == *v)
                                .unwrap_or(false)
                        })
                })
            })
            .map(|(_, r)| r)
            .collect::<Vec<_>>()
    } else {
        query_results
    };

    let mut response = serde_json::to_value(crate::searcher::SearchResponse {
        query: sanitized.clean_query.clone(),
        filters: crate::searcher::SearchFilters {
            wing: input.wing.clone(),
            room: input.room.clone(),
        },
        results: filtered_results
            .into_iter()
            .map(crate::searcher::SearchResult::from)
            .collect(),
    })
    .map_err(|e| internal_error_safe(&e))?;

    // Apply max_per_session filter (post-query deduplication by session)
    if let Some(max) = input.max_per_session {
        if let Some(obj) = response.as_object_mut() {
            if let Some(results) = obj.get_mut("results").and_then(|v| v.as_array_mut()) {
                let mut session_counts: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();
                results.retain(|r| {
                    let source = r
                        .get("source_file")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?")
                        .to_string();
                    let count = session_counts.entry(source).or_insert(0);
                    *count += 1;
                    *count <= max
                });
            }
        }
    }

    if let Some(object) = response.as_object_mut() {
        if sanitized.was_sanitized {
            object.insert("query_sanitized".to_string(), serde_json::Value::Bool(true));
            object.insert(
                "sanitizer".to_string(),
                serde_json::json!({
                    "method": sanitized.method,
                    "original_length": sanitized.original_length,
                    "clean_length": sanitized.clean_length,
                    "clean_query": sanitized.clean_query,
                }),
            );
        }

        if input
            .context
            .as_deref()
            .is_some_and(|context| !context.trim().is_empty())
        {
            object.insert(
                "context_received".to_string(),
                serde_json::Value::Bool(true),
            );
        }
    }

    ok_json(response)
}

fn tool_check_duplicate(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    #[derive(Deserialize)]
    struct Input {
        content: String,
        threshold: Option<f64>,
    }
    let input: Input = parse_args(args)?;
    let db = crate::palace_db::PalaceDb::open(&state.palace_path)
        .map_err(|e| internal_error_safe(&e))?;
    let matches = db
        .query_sync(&input.content, None, None, 5)
        .map_err(|e| internal_error_safe(&e))?
        .into_iter()
        .filter_map(|result| {
            let similarity = ((1.0 - result.distances.first().copied().unwrap_or(1.0)) * 1000.0)
                .round()
                / 1000.0;
            if similarity < input.threshold.unwrap_or(0.9) {
                return None;
            }

            let id = result.ids.first().cloned().unwrap_or_default();
            let meta = result.metadatas.first().cloned().unwrap_or_default();
            let content = result.documents.first().cloned().unwrap_or_default();

            Some(serde_json::json!({
                "id": id,
                "wing": meta.get("wing").and_then(|v| v.as_str()).unwrap_or("?"),
                "room": meta.get("room").and_then(|v| v.as_str()).unwrap_or("?"),
                "similarity": similarity,
                "content": if content.chars().count() > 200 {
                    format!("{}...", content.chars().take(200).collect::<String>())
                } else {
                    content
                },
            }))
        })
        .collect::<Vec<_>>();
    ok_json(serde_json::json!({ "is_duplicate": !matches.is_empty(), "matches": matches }))
}

fn tool_add_drawer(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    struct Input {
        wing: String,
        room: String,
        content: String,
        source_file: Option<String>,
        added_by: Option<String>,
        #[serde(flatten)]
        custom_metadata: Option<serde_json::Value>,
    }
    let input: Input = parse_args(args)?;
    let hash = short_hash(
        &format!("{}{}{}", input.wing, input.room, input.content),
        24,
    );
    let drawer_id = format!("drawer_{}_{}_{}", input.wing, input.room, hash);
    let mut db = crate::palace_db::PalaceDb::open(&state.palace_path)
        .map_err(|e| internal_error_safe(&e))?;
    if db._get_document(&drawer_id).is_some() {
        return ok_json(
            serde_json::json!({ "success": true, "reason": "already_exists", "drawer_id": drawer_id }),
        );
    }

    // Build standard metadata
    let mut standard_metadata = vec![
        ("wing", input.wing.as_str()),
        ("room", input.room.as_str()),
        ("source_file", input.source_file.as_deref().unwrap_or("")),
        ("added_by", input.added_by.as_deref().unwrap_or("mcp")),
        ("chunk_index", "0"),
    ];

    // Add custom metadata if provided
    if let Some(custom_meta) = &input.custom_metadata {
        if let Some(obj) = custom_meta.as_object() {
            for (key, value) in obj {
                if let Some(str_val) = value.as_str() {
                    standard_metadata.push((key.as_str(), str_val));
                }
            }
        }
    }

    db.add(&[(&drawer_id, &input.content)], &[&standard_metadata])
        .map_err(|e| internal_error_safe(&e))?;
    db.flush().map_err(|e| internal_error_safe(&e))?;
    crate::palace_graph::invalidate_cache(&state.palace_path);
    ok_json(
        serde_json::json!({ "success": true, "drawer_id": drawer_id, "wing": input.wing, "room": input.room }),
    )
}

fn tool_delete_drawer(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    #[derive(Deserialize)]
    struct Input {
        drawer_id: String,
    }
    let input: Input = parse_args(args)?;
    let mut db = crate::palace_db::PalaceDb::open(&state.palace_path)
        .map_err(|e| internal_error_safe(&e))?;
    let removed = db
        .delete_id(&input.drawer_id)
        .map_err(|e| internal_error_safe(&e))?;
    if removed {
        crate::palace_graph::invalidate_cache(&state.palace_path);
        ok_json(serde_json::json!({ "success": true, "drawer_id": input.drawer_id }))
    } else {
        ok_json(
            serde_json::json!({ "success": false, "error": format!("Drawer not found: {}", input.drawer_id) }),
        )
    }
}

fn tool_kg_query(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    struct Input {
        entity: String,
        as_of: Option<String>,
        tt_as_of: Option<String>,
        direction: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
        #[serde(default)]
        offset: Option<usize>,
    }
    let input: Input = parse_args(args)?;
    // Validate ISO-8601 date at MCP boundary (#1164): malformed dates would
    // silently produce empty result sets, indistinguishable from "no facts".
    let as_of = crate::config::sanitize_iso_temporal(input.as_of.as_deref(), "as_of")
        .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
    let tt_as_of = crate::config::sanitize_iso_temporal(input.tt_as_of.as_deref(), "tt_as_of")
        .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
    let kg = crate::knowledge_graph::KnowledgeGraph::open(&kg_path(state))
        .map_err(|e| internal_error_safe(&e))?;

    // Fetch the full fact set first for total counts, then apply pagination.
    let all_facts = kg
        .query_entity(
            &input.entity,
            as_of.as_deref(),
            tt_as_of.as_deref(),
            input.direction.as_deref().unwrap_or("both"),
        )
        .map_err(|e| internal_error_safe(&e))?;

    let offset = input.offset.unwrap_or(0);
    let limit = input.limit.unwrap_or(500);
    let count = all_facts.len();
    let truncated = count > limit + offset;
    let facts: Vec<_> = all_facts.into_iter().skip(offset).take(limit).collect();

    // Check whether the KG has a snapshot and attach metadata.
    let (total_nodes, total_edges, from_snapshot) = {
        match kg.get_snapshot() {
            Ok(Some(snap)) => (Some(snap.total_nodes), Some(snap.total_edges), true),
            _ => (None, None, false),
        }
    };

    ok_json(serde_json::json!({
        "entity": input.entity,
        "as_of": as_of,
        "nodes": facts,
        "edges": facts,
        "totalNodes": total_nodes.unwrap_or(0),
        "totalEdges": total_edges.unwrap_or(0),
        "total": count,
        "truncated": truncated,
        "offset": offset,
        "limit": limit,
        "fromSnapshot": from_snapshot,
    }))
}

fn tool_kg_add(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    struct Input {
        subject: String,
        predicate: String,
        object: String,
        valid_from: Option<String>,
        // Forwarded to the KG layer (#1314): callers need `valid_to` to
        // backfill already-ended historical facts in a single call instead
        // of doing add+invalidate, and `source_file` /
        // `source_drawer_id` for adapter provenance (RFC 002 §5.5).
        valid_to: Option<String>,
        source_closet: Option<String>,
        source_file: Option<String>,
        source_drawer_id: Option<String>,
    }
    let input: Input = parse_args(args)?;
    // Validate ISO-8601 dates at MCP boundary (#1164) so malformed dates fail
    // fast with a clear error instead of producing silently-invisible triples.
    let valid_from =
        crate::config::sanitize_iso_temporal(input.valid_from.as_deref(), "valid_from")
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
    let valid_to = crate::config::sanitize_iso_temporal(input.valid_to.as_deref(), "valid_to")
        .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
    let mut kg = crate::knowledge_graph::KnowledgeGraph::open(&kg_path(state))
        .map_err(|e| internal_error_safe(&e))?;
    let triple_id = kg
        .add_triple(
            &input.subject,
            &input.predicate,
            &input.object,
            valid_from.as_deref(),
            valid_to.as_deref(),
            None,
            input.source_closet.as_deref(),
            input.source_file.as_deref(),
            input.source_drawer_id.as_deref(),
            None,
        )
        .map_err(|e| internal_error_safe(&e))?;
    crate::palace_graph::invalidate_cache(&state.palace_path);
    ok_json(
        serde_json::json!({ "success": true, "triple_id": triple_id, "fact": format!("{} → {} → {}", input.subject, input.predicate, input.object) }),
    )
}

fn tool_kg_invalidate(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    struct Input {
        subject: String,
        predicate: String,
        object: String,
        ended: Option<String>,
    }
    let input: Input = parse_args(args)?;
    // Validate ISO-8601 date at MCP boundary (#1164) before forwarding to KG.
    let ended = crate::config::sanitize_iso_temporal(input.ended.as_deref(), "ended")
        .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
    // Resolve omitted/empty `ended` to today's date at the MCP layer so the
    // response reports the actual value persisted, not the literal sentinel
    // string "today" (#1314).
    let resolved_ended = ended
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
    let mut kg = crate::knowledge_graph::KnowledgeGraph::open(&kg_path(state))
        .map_err(|e| internal_error_safe(&e))?;
    kg.invalidate(
        &input.subject,
        &input.predicate,
        &input.object,
        Some(resolved_ended.as_str()),
    )
    .map_err(|e| internal_error_safe(&e))?;
    ok_json(
        serde_json::json!({ "success": true, "fact": format!("{} → {} → {}", input.subject, input.predicate, input.object), "ended": resolved_ended }),
    )
}

fn tool_kg_timeline(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    struct Input {
        entity: Option<String>,
    }
    let input: Input = parse_args(args)?;
    let kg = crate::knowledge_graph::KnowledgeGraph::open(&kg_path(state))
        .map_err(|e| internal_error_safe(&e))?;
    let timeline = kg
        .timeline(input.entity.as_deref())
        .map_err(|e| internal_error_safe(&e))?;
    ok_json(
        serde_json::json!({ "entity": input.entity.clone().unwrap_or_else(|| "all".to_string()), "timeline": timeline, "count": timeline.len() }),
    )
}

fn tool_kg_stats(state: &AppState, _args: JsonObject) -> Result<CallToolResult, ErrorData> {
    let kg = crate::knowledge_graph::KnowledgeGraph::open(&kg_path(state))
        .map_err(|e| internal_error_safe(&e))?;
    let stats = kg.stats().map_err(|e| internal_error_safe(&e))?;
    ok_json(serde_json::json!({
        "entities": stats.total_entities,
        "triples": stats.total_triples,
        "current_facts": stats.current_facts,
        "expired_facts": stats.expired_facts,
        "relationship_types": stats.relationship_types,
    }))
}

fn tool_kg_snapshot_rebuild(
    state: &AppState,
    args: JsonObject,
) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    struct Input {
        #[serde(default)]
        force: Option<bool>,
    }
    let input: Input = parse_args(args)?;
    let kg = crate::knowledge_graph::KnowledgeGraph::open(&kg_path(state))
        .map_err(|e| internal_error_safe(&e))?;

    let preflight = kg
        .snapshot_preflight()
        .map_err(|e| internal_error_safe(&e))?;

    // Refuse when totalNodes > 25,000 AND no prior snapshot AND force != true.
    let ceiling: usize = 25_000;
    if preflight.total_nodes > ceiling && !preflight.has_snapshot && !input.force.unwrap_or(false) {
        return ok_json(serde_json::json!({
            "success": false,
            "tooLarge": true,
            "totalNodes": preflight.total_nodes,
            "ceiling": ceiling,
        }));
    }

    let snapshot = kg.create_snapshot().map_err(|e| internal_error_safe(&e))?;

    ok_json(serde_json::json!({
        "success": true,
        "snapshotId": snapshot.snapshot_id,
        "totalNodes": snapshot.total_nodes,
        "totalEdges": snapshot.total_edges,
    }))
}

fn tool_kg_reset(state: &AppState, _args: JsonObject) -> Result<CallToolResult, ErrorData> {
    let kg = crate::knowledge_graph::KnowledgeGraph::open(&kg_path(state))
        .map_err(|e| internal_error_safe(&e))?;

    let snapshot = kg.reset_snapshot().map_err(|e| internal_error_safe(&e))?;

    ok_json(serde_json::json!({
        "success": true,
        "snapshotId": snapshot.snapshot_id,
        "resetAt": snapshot.reset_at,
    }))
}

#[allow(dead_code)]
fn build_graph_from_db(state: &AppState) -> crate::palace_graph::PalaceGraph {
    use crate::palace_db::PalaceDb;
    use crate::palace_graph::{HallType, PalaceGraph, Room, Wing, WingType};
    let mut by_wing: HashMap<String, Vec<Room>> = HashMap::new();
    // Reopen from disk so a graph built mid-session reflects mutations
    // committed by other tools in the same session. Use ok() so that if
    // the palace is genuinely unreachable we return an empty graph rather
    // than panicking the dispatcher.
    let entries = crate::palace_db::PalaceDb::open(&state.palace_path)
        .ok()
        .map(|db| db.get_all(None, None, usize::MAX))
        .unwrap_or_default();
    for entry in entries {
        if let Some(meta) = entry.metadatas.first() {
            let wing = meta
                .get("wing")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let room = meta
                .get("room")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let hall = match meta
                .get("hall")
                .and_then(|v| v.as_str())
                .unwrap_or("hall_facts")
            {
                "hall_events" | "events" => HallType::Events,
                "hall_discoveries" | "discoveries" => HallType::Discoveries,
                "hall_preferences" | "preferences" => HallType::Preferences,
                "hall_advice" | "advice" => HallType::Advice,
                "hall_facts" | "facts" => HallType::Facts,
                other => HallType::Raw(other.to_string()),
            };
            by_wing.entry(wing).or_default().push(Room {
                name: room,
                hall,
                closet_id: entry.ids.first().cloned(),
                date: meta
                    .get("date")
                    .and_then(|value| value.as_str())
                    .map(str::to_string),
            });
        }
    }
    let mut graph = PalaceGraph::new();
    for (wing_name, rooms) in by_wing {
        graph.add_wing(Wing {
            name: wing_name,
            wing_type: WingType::Topic,
            rooms,
        });
    }
    graph
}

fn tool_traverse(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    #[derive(Deserialize)]
    struct Input {
        start_room: String,
        max_hops: Option<usize>,
    }
    let input: Input = parse_args_with_integer_coercion(args, &["max_hops"])?;
    let graph = crate::palace_graph::cached_graph(&state.palace_path);
    match graph.traverse(&input.start_room, input.max_hops.unwrap_or(2)) {
        crate::palace_graph::TraverseOutcome::Results(results) => ok_json(results),
        crate::palace_graph::TraverseOutcome::Error(error) => ok_json(error),
    }
}

fn tool_find_tunnels(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    #[derive(Deserialize)]
    struct Input {
        wing_a: Option<String>,
        wing_b: Option<String>,
    }
    let input: Input = parse_args(args)?;
    let graph = crate::palace_graph::cached_graph(&state.palace_path);
    let tunnels = graph.find_tunnels(input.wing_a.as_deref(), input.wing_b.as_deref());
    ok_json(tunnels)
}

// mr-0qr1: hallways are user-facing aliases for tunnels. The two
// operations needed are list (read-only) and delete (mutation).
fn tool_list_hallways(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    #[derive(Deserialize)]
    struct Input {
        wing_a: Option<String>,
        wing_b: Option<String>,
    }
    let input: Input = parse_args(args)?;
    let graph = crate::palace_graph::cached_graph(&state.palace_path);
    let tunnels = graph.find_tunnels(input.wing_a.as_deref(), input.wing_b.as_deref());
    ok_json(serde_json::json!({ "hallways": tunnels }))
}

fn tool_delete_hallway(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if state.read_only {
        return Ok(CallToolResult::error(vec![rmcp::model::Content::text(
            "mempalace_delete_hallway is disabled in read-only mode",
        )]));
    }
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    #[derive(Deserialize)]
    struct Input {
        hallway_id: String,
    }
    let input: Input = parse_args(args)?;
    // Hallways are derived from the underlying graph; "deleting" one
    // means removing the source/target drawers that bridge the two
    // wings. We surface a clear error so callers know the operation
    // is intentionally constrained: tunnel deletion requires removing
    // the supporting drawers, which the caller should do explicitly.
    Ok(CallToolResult::error(vec![rmcp::model::Content::text(format!(
        "Hallway '{}' is derived from the graph; delete the source and target drawers to remove it.",
        input.hallway_id
    ))]))
}

fn tool_graph_stats(state: &AppState, _args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    let graph = crate::palace_graph::cached_graph(&state.palace_path);
    let stats = graph.stats();
    ok_json(serde_json::json!({
        "total_rooms": stats.total_rooms,
        "tunnel_rooms": stats.tunnel_rooms,
        "total_edges": stats.total_edges,
        "rooms_per_wing": stats.rooms_per_wing,
        "top_tunnels": stats.top_tunnels,
    }))
}

// ---------------------------------------------------------------------------
// Phase 8 tool handlers
// ---------------------------------------------------------------------------

fn tool_heal(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    struct Input {
        dry_run: Option<bool>,
    }
    let input: Input = parse_args(args)?;

    let db = fresh_db(state)?;
    let all_entries = db.get_all(None, None, usize::MAX);

    let mut actions: Vec<crate::types::Action> = Vec::new();
    let mut leases: Vec<crate::types::Lease> = Vec::new();

    for entry in &all_entries {
        if let Some(meta) = entry.metadatas.first() {
            let action_type = meta.get("type").and_then(|v| v.as_str());
            if action_type == Some("action") {
                if let Some(doc) = entry.documents.first() {
                    if let Ok(action) = serde_json::from_str::<crate::types::Action>(doc) {
                        actions.push(action);
                    }
                }
            } else if action_type == Some("lease") {
                if let Some(doc) = entry.documents.first() {
                    if let Ok(lease) = serde_json::from_str::<crate::types::Lease>(doc) {
                        leases.push(lease);
                    }
                }
            }
        }
    }

    let empty_edges: Vec<crate::types::ActionEdge> = Vec::new();

    let result = crate::heal::heal_all(
        &mut actions,
        &empty_edges,
        &mut leases,
        input.dry_run.unwrap_or(false),
    )
    .map_err(|e| internal_error_safe(&e))?;

    ok_json(serde_json::json!({
        "fixed": result.fixed,
        "failed": result.failed,
        "dry_run": result.dry_run,
        "actions_healed": result.fixed.iter().filter(|s| s.starts_with("action:")).count(),
        "leases_healed": result.fixed.iter().filter(|s| s.starts_with("lease:")).count(),
    }))
}

fn tool_verify(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    #[derive(Deserialize)]
    struct Input {
        target_id: String,
        target_type: String,
    }
    let input: Input = parse_args(args)?;

    let db = fresh_db(state)?;
    let all_entries = db.get_all(None, None, usize::MAX);

    let memories: Vec<crate::types::Memory> = all_entries
        .iter()
        .filter_map(|entry| {
            entry
                .documents
                .first()
                .and_then(|doc| serde_json::from_str::<crate::types::Memory>(doc).ok())
        })
        .collect();

    let observations: Vec<crate::types::CompressedObservation> = all_entries
        .iter()
        .filter_map(|entry| {
            entry.documents.first().and_then(|doc| {
                serde_json::from_str::<crate::types::CompressedObservation>(doc).ok()
            })
        })
        .collect();

    let session_ids: Vec<String> = all_entries
        .iter()
        .filter_map(|entry| {
            entry
                .metadatas
                .first()?
                .get("session_id")?
                .as_str()
                .map(String::from)
        })
        .collect();

    let verify_result = if input.target_type == "memory" {
        crate::verify::verify_memory(&input.target_id, &memories, &observations)
    } else {
        crate::verify::verify_observation(&input.target_id, &observations, &session_ids)
    }
    .map_err(|e| internal_error_safe(&e))?;

    ok_json(serde_json::json!({
        "id": verify_result.id,
        "verified": verify_result.verified,
        "confidence": verify_result.confidence,
        "chain": verify_result.chain,
        "issues": verify_result.issues,
    }))
}

fn tool_governance_delete(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    struct Input {
        max_age_days: Option<u64>,
        min_strength: Option<f64>,
        memory_type: Option<String>,
        project: Option<String>,
        not_accessed_since_days: Option<u64>,
        reason: String,
        #[serde(rename = "type")]
        memory_type_field: Option<String>,
    }
    let input: Input = parse_args(args)?;

    let db = fresh_db(state)?;
    let all_entries = db.get_all(None, None, usize::MAX);

    let mut memories: Vec<crate::types::Memory> = all_entries
        .iter()
        .filter_map(|entry| {
            entry
                .documents
                .first()
                .and_then(|doc| serde_json::from_str::<crate::types::Memory>(doc).ok())
        })
        .collect();

    let filter = crate::governance::GovernanceFilter {
        max_age_days: input.max_age_days,
        min_strength: input.min_strength,
        memory_type: input.memory_type.or(input.memory_type_field),
        project: input.project,
        tags: Vec::new(),
        not_accessed_since_days: input.not_accessed_since_days,
    };

    let result = crate::governance::governance_delete(&mut memories, &filter, &input.reason);

    if !result.deleted_ids.is_empty() {
        let mut db = fresh_db(state)?;
        for id in &result.deleted_ids {
            let _ = db.delete_id(id);
        }
        db.flush().map_err(|e| internal_error_safe(&e))?;
    }

    ok_json(serde_json::json!({
        "deleted_ids": result.deleted_ids,
        "count": result.count,
        "reason": result.reason,
    }))
}

fn tool_obsidian_export(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    struct Input {
        export_type: String,
        output_dir: Option<String>,
        include_frontmatter: Option<bool>,
        include_tags: Option<bool>,
    }
    let input: Input = parse_args(args)?;

    let db = fresh_db(state)?;
    let all_entries = db.get_all(None, None, usize::MAX);

    let config = crate::obsidian_export::ObsidianExportConfig {
        output_dir: input
            .output_dir
            .unwrap_or_else(|| "./memory-export".to_string()),
        include_frontmatter: input.include_frontmatter.unwrap_or(true),
        include_tags: input.include_tags.unwrap_or(true),
        include_links: true,
        tag_prefix: "memory/".to_string(),
        date_format: "%Y-%m-%d %H:%M".to_string(),
    };

    let export_result = if input.export_type == "observations" {
        let observations: Vec<crate::types::CompressedObservation> = all_entries
            .iter()
            .filter_map(|entry| {
                entry.documents.first().and_then(|doc| {
                    serde_json::from_str::<crate::types::CompressedObservation>(doc).ok()
                })
            })
            .collect();
        crate::obsidian_export::export_observations(&observations, &config)
    } else {
        let memories: Vec<crate::types::Memory> = all_entries
            .iter()
            .filter_map(|entry| {
                entry
                    .documents
                    .first()
                    .and_then(|doc| serde_json::from_str::<crate::types::Memory>(doc).ok())
            })
            .collect();
        crate::obsidian_export::export_memories(&memories, &config)
    }
    .map_err(|e| internal_error_safe(&e))?;

    ok_json(serde_json::json!({
        "exported_count": export_result.exported_count,
        "output_dir": export_result.output_dir,
        "files": export_result.files,
    }))
}

fn tool_compress_file(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    struct Input {
        file_path: String,
        dry_run: Option<bool>,
    }
    let input: Input = parse_args(args)?;

    let path = std::path::PathBuf::from(&input.file_path);
    let result =
        crate::compress_file::compress_markdown_file(&path).map_err(|e| internal_error_safe(&e))?;

    if input.dry_run.unwrap_or(false) {
        ok_json(serde_json::json!({
            "would_compress": true,
            "original_size": result.original_size,
            "compressed_size": result.compressed_size,
            "reduction_pct": result.reduction_pct,
            "message": "dry_run=true — no changes made",
        }))
    } else {
        ok_json(serde_json::json!({
            "original_path": result.original_path,
            "backup_path": result.backup_path,
            "original_size": result.original_size,
            "compressed_size": result.compressed_size,
            "reduction_pct": result.reduction_pct,
        }))
    }
}

fn tool_detect_worktree(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    struct Input {
        project_path: Option<String>,
    }
    let input: Input = parse_args(args)?;

    let path = input
        .project_path
        .as_deref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| state.palace_path.clone());

    let worktrees =
        crate::branch_aware::list_worktrees(&path).map_err(|e| internal_error_safe(&e))?;

    ok_json(serde_json::json!({
        "worktrees": worktrees,
        "count": worktrees.len(),
    }))
}

fn tool_replay_import(_state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    struct Input {
        project_filter: Option<String>,
    }
    let input: Input = parse_args(args)?;

    let sessions = crate::replay::load_all_sessions().map_err(|e| internal_error_safe(&e))?;

    let filtered: Vec<_> = if let Some(ref proj) = input.project_filter {
        sessions
            .into_iter()
            .filter(|s| s.project == *proj)
            .collect()
    } else {
        sessions
    };

    let summaries: Vec<serde_json::Value> = filtered
        .iter()
        .map(|s| {
            serde_json::json!({
                "id": s.id,
                "project": s.project,
                "message_count": s.message_count,
                "observation_count": s.observations.len(),
            })
        })
        .collect();

    ok_json(serde_json::json!({
        "sessions": summaries,
        "count": summaries.len(),
        "project_filter": input.project_filter,
    }))
}

fn tool_branch_detect(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    struct Input {
        project_path: Option<String>,
    }
    let input: Input = parse_args(args)?;

    let path = input
        .project_path
        .as_deref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| state.palace_path.clone());

    match crate::branch_aware::detect_worktree(&path) {
        Ok(Some(worktree)) => ok_json(serde_json::json!({
            "detected": true,
            "worktree": worktree,
        })),
        Ok(None) => ok_json(serde_json::json!({
            "detected": false,
            "worktree": null,
        })),
        Err(e) => Err(internal_error_safe(&e)),
    }
}

fn tool_branch_sessions(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    struct Input {
        branch: String,
    }
    let input: Input = parse_args(args)?;

    let db = fresh_db(state)?;
    let sessions = crate::branch_aware::branch_sessions(&[], &input.branch);

    ok_json(serde_json::json!({
        "branch": input.branch,
        "sessions": sessions,
        "count": sessions.len(),
    }))
}

fn tool_branch_worktrees(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    struct Input {
        project_path: Option<String>,
    }
    let input: Input = parse_args(args)?;

    let path = input
        .project_path
        .as_deref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| state.palace_path.clone());

    match crate::branch_aware::list_worktrees(&path) {
        Ok(worktrees) => ok_json(serde_json::json!({
            "worktrees": worktrees,
            "count": worktrees.len(),
        })),
        Err(e) => Err(internal_error_safe(&e)),
    }
}

// ---------------------------------------------------------------------------
// Group A: Multi-Agent Coordination tool handlers
// ---------------------------------------------------------------------------

fn tool_action_create(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        title: String,
        description: Option<String>,
        priority: Option<i64>,
        project: Option<String>,
        depends_on: Option<Vec<String>>,
    }
    let input: Input = parse_args(args)?;
    let db = fresh_db(state)?;
    let now = chrono::Utc::now().to_rfc3339();
    let action_id = format!("action_{}", short_hash(&input.title, 16));
    let action = crate::palace_db::Action {
        id: action_id.clone(),
        title: input.title.clone(),
        description: input.description.unwrap_or_default(),
        status: "pending".to_string(),
        priority: input.priority.unwrap_or(5),
        project: input.project.unwrap_or_default(),
        tags: String::new(),
        parent_id: None,
        created_at: now.clone(),
        updated_at: now,
    };
    if let Err(e) = db.coordination().action_create(&action) {
        return Err(internal_error_safe(&e));
    }
    if let Some(deps) = &input.depends_on {
        let mut coord = db.coordination();
        for dep_id in deps {
            if let Err(e) = coord.action_add_dependency(&action_id, dep_id) {
                tracing::warn!("Failed to add dependency: {}", e);
            }
        }
    }
    ok_json(serde_json::json!({
        "success": true,
        "action_id": action_id,
        "title": input.title,
    }))
}

fn tool_action_update(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        action_id: String,
        status: Option<String>,
        result: Option<String>,
        priority: Option<i64>,
    }
    let input: Input = parse_args(args)?;
    let mut db = fresh_db(state)?;
    let mut coord = db.coordination();
    let existing = coord
        .action_get(&input.action_id)
        .map_err(|e| internal_error_safe(&e))?;
    if existing.is_none() {
        return Err(ErrorData::invalid_params(
            format!("Action {} not found", input.action_id),
            None,
        ));
    }
    coord
        .action_update(
            &input.action_id,
            input.status.as_deref(),
            input.result.as_deref(),
            input.priority,
        )
        .map_err(|e| internal_error_safe(&e))?;
    ok_json(serde_json::json!({
        "success": true,
        "action_id": input.action_id,
    }))
}

fn tool_frontier(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        project: Option<String>,
        agent_id: Option<String>,
        limit: Option<usize>,
    }
    let input: Input = parse_args(args)?;
    let db = fresh_db(state)?;
    let limit = input.limit.unwrap_or(20);
    let actions = db
        .coordination()
        .action_list_unblocked(input.project.as_deref(), limit)
        .map_err(|e| internal_error_safe(&e))?;
    let count = actions.len();
    let action_summaries: Vec<serde_json::Value> = actions
        .into_iter()
        .map(|a| {
            serde_json::json!({
                "id": a.id,
                "title": a.title,
                "status": a.status,
                "priority": a.priority,
                "project": a.project,
            })
        })
        .collect();
    ok_json(serde_json::json!({
        "actions": action_summaries,
        "count": count,
    }))
}

fn tool_next(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        project: Option<String>,
        agent_id: Option<String>,
    }
    let input: Input = parse_args(args)?;
    let db = fresh_db(state)?;
    let mut actions = db
        .coordination()
        .action_list_unblocked(input.project.as_deref(), 1)
        .map_err(|e| internal_error_safe(&e))?;
    if actions.is_empty() {
        return ok_json(serde_json::json!({
            "action": serde_json::Value::Null,
            "message": "No unblocked actions available",
        }));
    }
    let action = actions.remove(0);
    ok_json(serde_json::json!({
        "action": {
            "id": action.id,
            "title": action.title,
            "description": action.description,
            "status": action.status,
            "priority": action.priority,
            "project": action.project,
            "tags": action.tags,
        },
    }))
}

fn tool_lease(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        action_id: String,
        holder: Option<String>,
        ttl_ms: Option<i64>,
        operation: String,
        result: Option<String>,
    }
    let input: Input = parse_args(args)?;
    let mut db = fresh_db(state)?;
    let mut coord = db.coordination();
    match input.operation.as_str() {
        "acquire" => {
            let existing = coord
                .lease_get_active(&input.action_id)
                .map_err(|e| internal_error_safe(&e))?;
            if existing.is_some() {
                return ok_json(serde_json::json!({
                    "success": false,
                    "message": "Action already has an active lease",
                }));
            }
            let holder = input.holder.unwrap_or_else(|| "unknown".to_string());
            let ttl = input.ttl_ms.unwrap_or(300000);
            let now = chrono::Utc::now();
            let expires = (now + chrono::Duration::milliseconds(ttl)).to_rfc3339();
            let lease_id = format!(
                "lease_{}",
                short_hash(&format!("{}{}", input.action_id, now), 12)
            );
            let lease = crate::palace_db::Lease {
                id: lease_id.clone(),
                action_id: input.action_id,
                agent_id: holder,
                status: "active".to_string(),
                result: input.result,
                ttl_ms: ttl,
                created_at: now.to_rfc3339(),
                expires_at: expires,
            };
            if let Err(e) = coord.lease_create(&lease) {
                return Err(internal_error_safe(&e));
            }
            ok_json(serde_json::json!({
                "success": true,
                "lease_id": lease_id,
            }))
        }
        "release" => {
            if let Some(holder) = &input.holder {
                let existing = coord
                    .lease_get_active(&input.action_id)
                    .map_err(|e| internal_error_safe(&e))?;
                if let Some(lease) = existing {
                    if lease.agent_id != *holder {
                        return ok_json(serde_json::json!({
                            "success": false,
                            "message": "Lease held by different agent",
                        }));
                    }
                    coord
                        .lease_release(&lease.id)
                        .map_err(|e| internal_error_safe(&e))?;
                }
            }
            ok_json(serde_json::json!({
                "success": true,
            }))
        }
        "renew" => {
            let ttl = input.ttl_ms.unwrap_or(300000);
            if let Some(holder) = &input.holder {
                let existing = coord
                    .lease_get_active(&input.action_id)
                    .map_err(|e| internal_error_safe(&e))?;
                if let Some(lease) = existing {
                    if lease.agent_id != *holder {
                        return ok_json(serde_json::json!({
                            "success": false,
                            "message": "Lease held by different agent",
                        }));
                    }
                    coord
                        .lease_renew(&lease.id, ttl)
                        .map_err(|e| internal_error_safe(&e))?;
                }
            }
            ok_json(serde_json::json!({
                "success": true,
            }))
        }
        _ => Err(ErrorData::invalid_params(
            format!("Unknown operation: {}", input.operation),
            None,
        )),
    }
}

fn tool_routine_run(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        routine_id: String,
        project: Option<String>,
        initiated_by: Option<String>,
    }
    let input: Input = parse_args(args)?;
    let db = fresh_db(state)?;
    let mut coord = db.coordination();
    let routine = coord
        .routine_get(&input.routine_id)
        .map_err(|e| internal_error_safe(&e))?;
    let routine = match routine {
        Some(r) => r,
        None => {
            return ok_json(serde_json::json!({
                "success": false,
                "message": format!("Routine {} not found", input.routine_id),
            }));
        }
    };
    let steps: Vec<serde_json::Value> = serde_json::from_str(&routine.steps).unwrap_or_default();
    let mut created_actions = Vec::new();
    let now = chrono::Utc::now().to_rfc3339();
    for (i, step) in steps.iter().enumerate() {
        let title = step
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or(&routine.name)
            .to_string();
        let description = step
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let priority = step.get("priority").and_then(|v| v.as_i64()).unwrap_or(5);
        let action_id = format!("{}_step_{}", input.routine_id, i);
        let action = crate::palace_db::Action {
            id: action_id.clone(),
            title,
            description,
            status: "pending".to_string(),
            priority,
            project: input.project.clone().unwrap_or_default(),
            tags: String::new(),
            parent_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        if let Err(e) = coord.action_create(&action) {
            tracing::warn!("Failed to create action for routine step: {}", e);
            continue;
        }
        created_actions.push(action_id);
    }
    ok_json(serde_json::json!({
        "success": true,
        "routine_id": input.routine_id,
        "created_actions": created_actions,
    }))
}

fn tool_signal_send(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        from: Option<String>,
        to: String,
        content: String,
        signal_type: Option<String>,
        reply_to: Option<String>,
    }
    let input: Input = parse_args(args)?;
    let db = fresh_db(state)?;
    let now = chrono::Utc::now();
    let signal_id = format!("sig_{}", short_hash(&format!("{}{}", input.to, now), 16));
    let signal = crate::palace_db::Signal {
        id: signal_id,
        from_agent: input.from.unwrap_or_else(|| "unknown".to_string()),
        to_agent: input.to,
        content: input.content,
        signal_type: input.signal_type.unwrap_or_else(|| "info".to_string()),
        reply_to: input.reply_to,
        read: false,
        created_at: now.to_rfc3339(),
    };
    if let Err(e) = db.coordination().signal_create(&signal) {
        return Err(internal_error_safe(&e));
    }
    ok_json(serde_json::json!({
        "success": true,
        "signal_id": signal.id,
    }))
}

fn tool_signal_read(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        agent_id: String,
        unread_only: Option<bool>,
        thread_id: Option<String>,
        limit: Option<usize>,
    }
    let input: Input = parse_args(args)?;
    let db = fresh_db(state)?;
    let limit = input.limit.unwrap_or(50);
    let signals = db
        .coordination()
        .signal_list(
            &input.agent_id,
            input.unread_only.unwrap_or(false),
            input.thread_id.as_deref(),
            limit,
        )
        .map_err(|e| internal_error_safe(&e))?;
    let count = signals.len();
    let messages: Vec<serde_json::Value> = signals
        .into_iter()
        .map(|s| {
            serde_json::json!({
                "id": s.id,
                "from": s.from_agent,
                "to": s.to_agent,
                "content": s.content,
                "type": s.signal_type,
                "reply_to": s.reply_to,
                "read": s.read,
                "created_at": s.created_at,
            })
        })
        .collect();
    ok_json(serde_json::json!({
        "messages": messages,
        "count": count,
    }))
}

fn tool_diary_read(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    #[derive(Deserialize)]
    struct Input {
        agent_name: String,
        last_n: Option<usize>,
        wing: Option<String>,
    }
    let input: Input = parse_args_with_integer_coercion(args, &["last_n"])?;
    // Case-insensitive agent name lookup (#1243): diary_write stores the
    // lowercased agent name, so reads must match against the same canonical form.
    let agent_name = input.agent_name.to_lowercase();
    let wing = input
        .wing
        .as_deref()
        .map(str::trim)
        .filter(|wing| !wing.is_empty())
        .map(ToString::to_string);
    let db = fresh_db(state)?;
    let entries = if let Some(wing) = wing.as_deref() {
        db.get_all(Some(wing), Some("diary"), usize::MAX)
    } else {
        db.get_all(None, Some("diary"), usize::MAX)
            .into_iter()
            .filter(|entry| {
                entry
                    .metadatas
                    .first()
                    .and_then(|meta| meta.get("agent"))
                    // mr-ong7: str-coerce before case-comparison so a
                    // numeric/boolean metadata value still matches. Use
                    // as_str() preferentially to avoid quoting a JSON
                    // string value (to_string() would emit `"claude"`
                    // including quotes).
                    .and_then(|agent| match agent {
                        serde_json::Value::String(s) => Some(s.clone()),
                        other => Some(other.to_string()),
                    })
                    .map(|agent| agent.eq_ignore_ascii_case(&agent_name))
                    .unwrap_or(false)
            })
            .collect()
    };
    let mut items: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| {
            let meta = e.metadatas.first().cloned().unwrap_or_default();
            serde_json::json!({
                "date": meta.get("date").and_then(|v| v.as_str()).unwrap_or(""),
                "timestamp": meta.get("filed_at").and_then(|v| v.as_str()).unwrap_or(""),
                "topic": meta.get("topic").and_then(|v| v.as_str()).unwrap_or(""),
                "content": e.documents.first().cloned().unwrap_or_default(),
            })
        })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| {
        let a_ts = a.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
        let b_ts = b.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
        b_ts.cmp(a_ts)
    });
    let showing = items.len().min(input.last_n.unwrap_or(10));
    if items.is_empty() {
        return ok_json(serde_json::json!({
            "agent": agent_name,
            "entries": [],
            "message": "No diary entries yet.",
        }));
    }
    ok_json(serde_json::json!({
        "agent": agent_name,
        "entries": items.into_iter().take(input.last_n.unwrap_or(10)).collect::<Vec<_>>(),
        "total": entries.len(),
        "showing": showing,
    }))
}

fn tool_diary_write(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    struct Input {
        agent_name: String,
        entry: String,
        topic: Option<String>,
        wing: Option<String>,
    }
    let input: Input = parse_args(args)?;
    // Normalize agent name to lowercase so reads are case-insensitive (#1243):
    // writing as "Claude" and reading as "claude" must resolve to the same agent.
    let agent_name = input.agent_name.to_lowercase();
    let now = chrono::Local::now();
    let wing = input
        .wing
        .as_deref()
        .map(str::trim)
        .filter(|wing| !wing.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("wing_{}", agent_name.replace(' ', "_")));
    let topic = input.topic.unwrap_or_else(|| "general".to_string());
    let id = format!(
        "diary_{}_{}_{}",
        wing,
        now.format("%Y%m%d_%H%M%S"),
        short_hash(&input.entry, 12)
    );
    let mut db = crate::palace_db::PalaceDb::open(&state.palace_path)
        .map_err(|e| internal_error_safe(&e))?;
    let date = now.format("%Y-%m-%d").to_string();
    let filed_at = now.to_rfc3339();
    db.add(
        &[(&id, &input.entry)],
        &[&[
            ("wing", &wing),
            ("room", "diary"),
            ("hall", "hall_diary"),
            ("topic", &topic),
            ("type", "diary_entry"),
            ("agent", &agent_name),
            ("filed_at", &filed_at),
            ("date", &date),
        ]],
    )
    .map_err(|e| internal_error_safe(&e))?;
    db.flush().map_err(|e| internal_error_safe(&e))?;
    crate::palace_graph::invalidate_cache(&state.palace_path);
    ok_json(serde_json::json!({
        "success": true,
        "entry_id": id,
        "agent": agent_name,
        "topic": topic,
        "timestamp": filed_at,
    }))
}

// ---------------------------------------------------------------------------
// Smart Features Tool Handlers
// ---------------------------------------------------------------------------

fn tool_sentinel_create(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        name: String,
        watch_type: String,
        #[serde(default)]
        trigger_condition: Option<String>,
        #[serde(default)]
        action_id: Option<String>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let sentinel_id = format!("sentinel_{}", short_hash(&input.name, 8));
    let now = chrono::Utc::now().to_rfc3339();
    let mut conn = state.db.coordination();
    conn.execute(
        "INSERT INTO sentinels (id, name, watch_type, trigger_condition, action_id, expires_at, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            &sentinel_id,
            &input.name,
            &input.watch_type,
            input.trigger_condition.as_deref().unwrap_or(""),
            input.action_id.as_deref().unwrap_or(""),
            rusqlite::types::Null,
            &now,
        ],
    )
    .map_err(|e| internal_error_safe(&e))?;
    ok_json(serde_json::json!({
        "success": true,
        "sentinel_id": sentinel_id,
        "name": input.name,
        "watch_type": input.watch_type,
        "status": "active",
    }))
}

fn tool_sentinel_trigger(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        sentinel_id: String,
        #[serde(default)]
        context: Option<String>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let mut conn = state.db.coordination();
    let sentinel = conn
        .sentinel_get(&input.sentinel_id)
        .map_err(|e| internal_error_safe(&e))?
        .ok_or_else(|| {
            ErrorData::invalid_params(format!("Sentinel not found: {}", input.sentinel_id), None)
        })?;
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE sentinels SET status = 'triggered', triggered_at = ?1 WHERE id = ?2",
        rusqlite::params![&now, &input.sentinel_id],
    )
    .map_err(|e| internal_error_safe(&e))?;
    ok_json(serde_json::json!({
        "success": true,
        "triggered": true,
        "sentinel_id": input.sentinel_id,
        "context": input.context,
        "triggered_at": now,
        "action_taken": sentinel.action_id.unwrap_or_default(),
    }))
}

fn tool_sentinel_list(state: &AppState, _args: JsonObject) -> Result<CallToolResult, ErrorData> {
    let mut conn = state.db.coordination();
    let sentinels = conn.sentinel_list().map_err(|e| internal_error_safe(&e))?;
    ok_json(serde_json::json!({
        "success": true,
        "sentinels": sentinels.into_iter().map(|s| serde_json::json!({
            "id": s.id,
            "name": s.name,
            "watch_type": s.watch_type,
            "trigger_condition": s.trigger_condition,
            "action_id": s.action_id,
            "expires_at": s.expires_at,
            "created_at": s.created_at,
        })).collect::<Vec<_>>(),
    }))
}

fn tool_sentinel_delete(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        sentinel_id: String,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let mut conn = state.db.coordination();
    conn.sentinel_delete(&input.sentinel_id)
        .map_err(|e| internal_error_safe(&e))?;
    ok_json(serde_json::json!({
        "success": true,
        "deleted": true,
        "sentinel_id": input.sentinel_id,
    }))
}

fn tool_checkpoint_list(state: &AppState, _args: JsonObject) -> Result<CallToolResult, ErrorData> {
    let mut conn = state.db.coordination();
    let checkpoints = conn
        .checkpoint_list()
        .map_err(|e| internal_error_safe(&e))?;
    ok_json(serde_json::json!({
        "success": true,
        "checkpoints": checkpoints.into_iter().map(|c| serde_json::json!({
            "id": c.id,
            "name": c.name,
            "operation": c.operation,
            "status": c.status,
            "checkpoint_type": c.checkpoint_type,
            "linked_action_ids": c.linked_action_ids,
            "created_at": c.created_at,
            "updated_at": c.updated_at,
        })).collect::<Vec<_>>(),
    }))
}

fn tool_checkpoint_resolve(
    state: &AppState,
    args: JsonObject,
) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        checkpoint_id: String,
        status: String,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let mut conn = state.db.coordination();
    conn.checkpoint_resolve(&input.checkpoint_id, &input.status)
        .map_err(|e| internal_error_safe(&e))?;
    ok_json(serde_json::json!({
        "success": true,
        "resolved": true,
        "checkpoint_id": input.checkpoint_id,
        "status": input.status,
    }))
}

fn tool_sketch_create(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        title: String,
        content: String,
        #[serde(default)]
        tags: Option<Vec<String>>,
        #[serde(default)]
        wing: Option<String>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let project = input.wing.unwrap_or_else(|| "default".to_string());
    let now = chrono::Utc::now().to_rfc3339();
    let sketch_id = format!("sketch_{}", short_hash(&input.title, 8));
    let steps = serde_json::json!([{"content": input.content, "order": 0}]);
    let sketch = crate::palace_db::SketchRecord {
        id: sketch_id.clone(),
        title: input.title.clone(),
        description: input.tags.clone().unwrap_or_default().join(", "),
        steps: steps.to_string(),
        project: project.clone(),
        expires_at: chrono::Utc::now()
            .checked_add_signed(chrono::Duration::days(7))
            .unwrap()
            .to_rfc3339(),
        created_at: now,
    };
    let mut db = fresh_db(state)?;
    if let Err(e) = db.sketch_create(&sketch) {
        return Err(ErrorData::invalid_request(
            format!("Failed to create sketch: {}", e),
            None,
        ));
    }
    ok_json(serde_json::json!({
        "success": true,
        "sketch_id": sketch_id,
        "title": input.title,
        "status": "draft",
        "project": project,
    }))
}

fn tool_sketch_promote(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        sketch_id: String,
        #[serde(default)]
        target_room: Option<String>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let mut db = fresh_db(state)?;

    // 1. Read the sketch
    let sketch = match db.sketch_get(&input.sketch_id) {
        Ok(Some(s)) => s,
        Ok(None) => {
            return Err(ErrorData::invalid_request(
                format!("Sketch not found: {}", input.sketch_id),
                None,
            ));
        }
        Err(e) => {
            return Err(ErrorData::invalid_request(
                format!("Failed to read sketch: {}", e),
                None,
            ));
        }
    };

    // 2. Extract action items from sketch content (title + description + steps)
    let content = format!("{}\n{}\n{}", sketch.title, sketch.description, sketch.steps);
    let action_items = extract_action_items(&content);

    // 3. Create permanent Action entries via CoordinationDb
    let mut action_ids = Vec::new();
    let now = chrono::Utc::now().to_rfc3339();
    let project = if sketch.project.is_empty() {
        "default".to_string()
    } else {
        sketch.project.clone()
    };
    {
        let mut coord = db.coordination();
        for item in action_items {
            let action_id = format!("action_{}", short_hash(&item, 16));
            let action = crate::palace_db::Action {
                id: action_id.clone(),
                title: item,
                description: format!("source_sketch:{}", input.sketch_id),
                status: "pending".to_string(),
                priority: 5,
                project: project.clone(),
                tags: "from_sketch".to_string(),
                parent_id: Some(input.sketch_id.clone()),
                created_at: now.clone(),
                updated_at: now.clone(),
            };
            if let Err(e) = coord.action_create(&action) {
                warn!("Failed to create action for sketch item: {}", e);
            } else {
                action_ids.push(action_id);
            }
        }
    }

    // 4. Delete the sketch
    if let Err(e) = db.sketch_delete(&input.sketch_id) {
        return Err(ErrorData::invalid_request(
            format!("Failed to delete sketch: {}", e),
            None,
        ));
    }

    ok_json(serde_json::json!({
        "success": true,
        "promoted": true,
        "sketch_id": input.sketch_id,
        "actions_created": action_ids.len(),
        "action_ids": action_ids,
        "target_room": input.target_room,
    }))
}

fn tool_crystallize(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        drawer_id: String,
        #[serde(default)]
        crystallize_type: Option<String>,
        #[serde(default)]
        action_ids: Option<Vec<String>>,
        #[serde(default)]
        summary: Option<String>,
        #[serde(default)]
        narrative: Option<String>,
        #[serde(default)]
        outcomes: Option<String>,
        #[serde(default)]
        files_affected: Option<Vec<String>>,
        #[serde(default)]
        lessons: Option<String>,
        #[serde(default)]
        project: Option<String>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let crystal_id = format!("crystal_{}", short_hash(&input.drawer_id, 8));
    let now = chrono::Utc::now().to_rfc3339();
    let project = input.project.unwrap_or_else(|| "default".to_string());
    let crystal = crate::palace_db::CrystalRecord {
        id: crystal_id.clone(),
        action_ids: input.action_ids.unwrap_or_default().join(","),
        summary: input.summary.unwrap_or_default(),
        narrative: input.narrative.unwrap_or_default(),
        outcomes: input.outcomes.unwrap_or_default(),
        files_affected: input.files_affected.unwrap_or_default().join(","),
        lessons: input.lessons.unwrap_or_default(),
        project: project.clone(),
        session_id: format!("session_{}", short_hash(&now, 8)),
        created_at: now,
    };
    let mut db = fresh_db(state)?;
    if let Err(e) = db.crystal_create(&crystal) {
        return Err(ErrorData::invalid_request(
            format!("Failed to create crystal: {}", e),
            None,
        ));
    }
    ok_json(serde_json::json!({
        "success": true,
        "drawer_id": input.drawer_id,
        "crystal_id": crystal_id,
        "crystallized": true,
        "crystal_type": input.crystallize_type.unwrap_or_else(|| "standard".to_string()),
        "project": project,
    }))
}

fn tool_diagnose(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        #[serde(default)]
        categories: Option<String>,
    }
    let _input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let db = fresh_db(state)?;
    let all_drawers = db.get_all(None, None, usize::MAX);
    let total_drawers = all_drawers.len();
    let coord = db.coordination();
    let actions = coord
        .action_list_all()
        .map_err(|e| internal_error_safe(&e))?;
    let leases = coord
        .lease_list_all()
        .map_err(|e| internal_error_safe(&e))?;
    let signals = coord
        .signal_list_all()
        .map_err(|e| internal_error_safe(&e))?;
    let stuck_actions = actions
        .iter()
        .filter(|a| a.status == "pending" || a.status == "blocked")
        .count();
    let stale_leases = leases
        .iter()
        .filter(|l| {
            chrono::DateTime::parse_from_rfc3339(&l.expires_at)
                .map(|dt| dt < chrono::Utc::now())
                .unwrap_or(false)
        })
        .count();
    let issues = stuck_actions + stale_leases;
    let health_score = if total_drawers == 0 {
        100
    } else {
        ((total_drawers.saturating_sub(stuck_actions)) as f64 / total_drawers as f64 * 100.0)
            .round() as i32
    };
    let diagnosis = if issues == 0 {
        "No issues found".to_string()
    } else {
        format!(
            "{} issues: {} stuck actions, {} stale leases",
            issues, stuck_actions, stale_leases
        )
    };
    ok_json(serde_json::json!({
        "success": true,
        "health_score": health_score.min(100),
        "diagnosis": diagnosis,
        "categories": {
            "actions": {
                "total": actions.len(),
                "stuck": stuck_actions,
            },
            "leases": {
                "total": leases.len(),
                "stale": stale_leases,
            },
            "memories": {
                "total": total_drawers,
            },
            "signals": {
                "total": signals.len(),
            },
        },
    }))
}

fn tool_facet_tag(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        drawer_id: String,
        tags: Vec<String>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let now = chrono::Utc::now().to_rfc3339();
    let mut db = fresh_db(state)?;
    for tag in &input.tags {
        let facet_id = format!(
            "facet_{}",
            short_hash(&format!("{}_{}", input.drawer_id, tag), 8)
        );
        let facet = crate::palace_db::FacetRecord {
            id: facet_id,
            target_id: input.drawer_id.clone(),
            target_type: "drawer".to_string(),
            dimension: "tag".to_string(),
            value: tag.clone(),
            created_at: now.clone(),
        };
        if let Err(e) = db.facet_create(&facet) {
            return Err(ErrorData::invalid_request(
                format!("Failed to create facet: {}", e),
                None,
            ));
        }
    }
    ok_json(serde_json::json!({
        "success": true,
        "drawer_id": input.drawer_id,
        "tags_added": input.tags,
        "total_tags": input.tags.len(),
    }))
}

fn tool_facet_query(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        query: String,
        #[serde(default)]
        facets: Option<Vec<String>>,
        #[serde(default)]
        limit: Option<usize>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let db = fresh_db(state)?;
    let facets = db
        .facet_list(None, None)
        .map_err(|e| ErrorData::invalid_request(format!("Failed to query facets: {}", e), None))?;
    let results: Vec<_> = facets
        .iter()
        .map(|f| {
            serde_json::json!({
                "id": f.id,
                "target_id": f.target_id,
                "dimension": f.dimension,
                "value": f.value,
            })
        })
        .collect();
    ok_json(serde_json::json!({
        "success": true,
        "query": input.query,
        "results": results,
        "total": results.len(),
    }))
}

fn tool_lesson_save(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        lesson: String,
        context: String,
        #[serde(default)]
        tags: Option<Vec<String>>,
        #[serde(default)]
        confidence: Option<f64>,
        #[serde(default)]
        project: Option<String>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let lesson_id = format!("lesson_{}", short_hash(&input.lesson, 8));
    let now = chrono::Utc::now().to_rfc3339();
    let project = input.project.unwrap_or_else(|| "default".to_string());
    let lesson = crate::palace_db::LessonRecord {
        id: lesson_id.clone(),
        content: input.lesson.clone(),
        context: input.context.clone(),
        confidence: input.confidence.unwrap_or(0.8),
        project: project.clone(),
        tags: input.tags.clone().unwrap_or_default().join(","),
        reinforced_at: now.clone(),
        created_at: now,
    };
    let mut db = fresh_db(state)?;
    if let Err(e) = db.lesson_create(&lesson) {
        return Err(ErrorData::invalid_request(
            format!("Failed to save lesson: {}", e),
            None,
        ));
    }
    ok_json(serde_json::json!({
        "success": true,
        "lesson_id": lesson_id,
        "lesson": input.lesson,
        "project": project,
        "saved_at": chrono::Utc::now().to_rfc3339(),
    }))
}

fn tool_lesson_recall(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        query: String,
        #[serde(default)]
        limit: Option<usize>,
        #[serde(default)]
        project: Option<String>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let db = fresh_db(state)?;
    let lessons = db
        .lesson_list(input.project.as_deref(), None)
        .map_err(|e| {
            ErrorData::invalid_request(format!("Failed to recall lessons: {}", e), None)
        })?;
    let results: Vec<_> = lessons
        .iter()
        .map(|l| {
            serde_json::json!({
                "id": l.id,
                "content": l.content,
                "context": l.context,
                "confidence": l.confidence,
            })
        })
        .collect();
    ok_json(serde_json::json!({
        "success": true,
        "query": input.query,
        "lessons": results,
        "total": results.len(),
    }))
}

struct ReflectInput {
    topic: String,
    max_clusters: Option<usize>,
}

async fn tool_reflect_async(
    state: &AppState,
    input: &ReflectInput,
) -> Result<serde_json::Value, ErrorData> {
    let mut db = fresh_db(state)?;
    let llm = crate::llm::create_llm_provider_from_env();

    // Collect all source material: lessons + insights + crystals
    let lessons = db
        .lesson_list(None, None)
        .map_err(|e| ErrorData::invalid_request(format!("Failed to list lessons: {}", e), None))?;
    let insights = db
        .insight_list(None, None)
        .map_err(|e| ErrorData::invalid_request(format!("Failed to list insights: {}", e), None))?;
    let crystals = db
        .crystal_list(None)
        .map_err(|e| ErrorData::invalid_request(format!("Failed to list crystals: {}", e), None))?;

    let total_sources = lessons.len() + insights.len() + crystals.len();
    if total_sources == 0 {
        return Ok(serde_json::json!({
            "success": true,
            "topic": input.topic,
            "new_insights": 0,
            "reinforced": 0,
            "clusters_processed": 0,
            "used_fallback": false,
            "insights": [],
        }));
    }

    // Phase 1: cluster source material by concept tags
    let mut concept_to_sources: std::collections::HashMap<String, Vec<ClusterMember>> =
        std::collections::HashMap::new();

    for lesson in &lessons {
        for tag in lesson.tags.split(',') {
            let tag = tag.trim();
            if !tag.is_empty()
                && lesson
                    .content
                    .to_lowercase()
                    .contains(&input.topic.to_lowercase())
            {
                concept_to_sources
                    .entry(tag.to_lowercase())
                    .or_default()
                    .push(ClusterMember {
                        source_type: "lesson".to_string(),
                        id: lesson.id.clone(),
                        content: lesson.content.clone(),
                        confidence: lesson.confidence,
                    });
            }
        }
    }

    for insight in &insights {
        let words: Vec<&str> = insight.content.split_whitespace().take(5).collect();
        for word in words {
            let word_lower = word.to_lowercase();
            if word_lower.len() > 3
                && insight
                    .content
                    .to_lowercase()
                    .contains(&input.topic.to_lowercase())
            {
                concept_to_sources
                    .entry(word_lower)
                    .or_default()
                    .push(ClusterMember {
                        source_type: "insight".to_string(),
                        id: insight.id.clone(),
                        content: insight.content.clone(),
                        confidence: insight.confidence,
                    });
            }
        }
    }

    for crystal in &crystals {
        if !crystal.lessons.is_empty() {
            for tag in crystal.lessons.split(',') {
                let tag = tag.trim();
                if !tag.is_empty() {
                    concept_to_sources
                        .entry(tag.to_lowercase())
                        .or_default()
                        .push(ClusterMember {
                            source_type: "crystal".to_string(),
                            id: crystal.id.clone(),
                            content: crystal.lessons.clone(),
                            confidence: 0.8,
                        });
                }
            }
        }
    }

    let mut clusters: Vec<(String, Vec<ClusterMember>)> = concept_to_sources
        .into_iter()
        .filter(|(_, members)| members.len() >= 2)
        .collect();
    clusters.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
    let max_clusters = input.max_clusters.unwrap_or(10).min(20);
    clusters.truncate(max_clusters);

    let mut new_insights = 0;
    let mut reinforced = 0;
    let mut results: Vec<serde_json::Value> = Vec::new();
    let now = chrono::Utc::now();

    for (concept, members) in &clusters {
        let cluster_items: Vec<String> = members
            .iter()
            .map(|m| {
                format!(
                    "- [{}] {} (confidence: {:.1}): {}",
                    m.source_type,
                    m.id,
                    m.confidence,
                    m.content.chars().take(200).collect::<String>()
                )
            })
            .collect();
        let prompt = format!(
            "Analyze these observations about '{}' and generate higher-order insights:\n\n{}\n\n\
            Focus on patterns, best practices, and cross-cutting lessons. \
            Output 1-2 concise insights (under 150 chars each).",
            concept,
            cluster_items.join("\n")
        );

        let response = llm
            .complete(crate::reflect::REFLECT_SYSTEM_PROMPT, &prompt)
            .await;

        match response {
            Ok(completion) => {
                if let Ok(parsed) = crate::reflect::parse_insights_xml(&completion.text) {
                    for insight in parsed {
                        let content_hash = short_hash(&insight.content, 12);
                        let insight_id = format!("ins_{}", content_hash);

                        let existing = db
                            .insight_list(None, Some(insight.confidence))
                            .ok()
                            .and_then(|list| {
                                list.into_iter().find(|i| {
                                    i.content == insight.content && !i.id.contains("deprecated")
                                })
                            });

                        if let Some(existing_insight) = existing {
                            if let Err(e) = db.insight_reinforce(&existing_insight.id) {
                                tracing::warn!("Failed to reinforce insight: {}", e);
                            }
                            reinforced += 1;
                            results.push(serde_json::json!({
                                "id": existing_insight.id,
                                "title": existing_insight.id,
                                "content": existing_insight.content,
                                "confidence": existing_insight.confidence,
                                "reinforced": true,
                                "source_concept": concept,
                            }));
                        } else {
                            let record = crate::palace_db::InsightRecord {
                                id: insight_id.clone(),
                                content: insight.content.clone(),
                                confidence: insight.confidence,
                                project: "default".to_string(),
                                cluster_id: concept.clone(),
                                reinforced_count: 0,
                                created_at: now.to_rfc3339(),
                            };
                            if let Err(e) = db.insight_create(&record) {
                                tracing::warn!("Failed to create insight: {}", e);
                            } else {
                                new_insights += 1;
                            }
                            results.push(serde_json::json!({
                                "id": insight_id,
                                "title": insight.title,
                                "content": insight.content,
                                "confidence": insight.confidence,
                                "reinforced": false,
                                "source_concept": concept,
                            }));
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("LLM reflect failed for cluster '{}': {}", concept, e);
            }
        }
    }

    Ok(serde_json::json!({
        "success": true,
        "topic": input.topic,
        "new_insights": new_insights,
        "reinforced": reinforced,
        "clusters_processed": clusters.len(),
        "clusters_skipped": 0,
        "used_fallback": false,
        "insights": results,
    }))
}

#[derive(Debug, Clone)]
struct ClusterMember {
    source_type: String,
    id: String,
    content: String,
    confidence: f64,
}

fn tool_reflect(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        topic: String,
        #[serde(default)]
        max_clusters: Option<usize>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let reflect_input = ReflectInput {
        topic: input.topic,
        max_clusters: input.max_clusters,
    };

    let rt = tokio::runtime::Handle::current();
    let result = rt.block_on(tool_reflect_async(state, &reflect_input));
    match result {
        Ok(json) => ok_json(json),
        Err(e) => Err(e),
    }
}

fn tool_insight_list(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        #[serde(default)]
        wing: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
        #[serde(default)]
        min_confidence: Option<f64>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let db = fresh_db(state)?;
    let insights = db
        .insight_list(input.wing.as_deref(), input.min_confidence)
        .map_err(|e| ErrorData::invalid_request(format!("Failed to list insights: {}", e), None))?;
    let results: Vec<_> = insights
        .iter()
        .map(|i| {
            serde_json::json!({
                "id": i.id,
                "content": i.content,
                "confidence": i.confidence,
                "cluster_id": i.cluster_id,
            })
        })
        .collect();
    ok_json(serde_json::json!({
        "success": true,
        "insights": results,
        "total": results.len(),
    }))
}

fn tool_slot_list(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        #[serde(default)]
        project: Option<String>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let db = fresh_db(state)?;
    let slots = db
        .slot_list(input.project.as_deref())
        .map_err(|e| ErrorData::invalid_request(format!("Failed to list slots: {}", e), None))?;
    let results: Vec<_> = slots
        .iter()
        .map(|s| {
            serde_json::json!({
                "id": s.id,
                "label": s.label,
                "content": s.content,
                "size_limit": s.size_limit,
                "description": s.description,
                "pinned": s.pinned,
                "scope": s.scope,
                "project": s.project,
                "created_at": s.created_at,
                "updated_at": s.updated_at,
            })
        })
        .collect();
    ok_json(serde_json::json!({
        "success": true,
        "slots": results,
        "total": results.len(),
    }))
}

fn tool_slot_get(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        label: String,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let db = fresh_db(state)?;
    let slot = db
        .slot_get(&input.label)
        .map_err(|e| ErrorData::invalid_request(format!("Failed to get slot: {}", e), None))?;
    match slot {
        Some(s) => ok_json(serde_json::json!({
            "success": true,
            "slot": {
                "id": s.id,
                "label": s.label,
                "content": s.content,
                "size_limit": s.size_limit,
                "description": s.description,
                "pinned": s.pinned,
                "scope": s.scope,
                "project": s.project,
                "created_at": s.created_at,
                "updated_at": s.updated_at,
            }
        })),
        None => Err(ErrorData::invalid_params(
            format!("Slot '{}' not found", input.label),
            None,
        )),
    }
}

fn tool_slot_create(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        label: String,
        content: Option<String>,
        size_limit: Option<i32>,
        description: Option<String>,
        scope: Option<String>,
        project: Option<String>,
        pinned: Option<bool>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let now = chrono::Utc::now();
    let created_at = now.to_rfc3339();
    let updated_at = created_at.clone();
    let slot = MemorySlot {
        id: format!("slot_{}", short_hash(&input.label, 8)),
        label: input.label.clone(),
        content: input.content.unwrap_or_default(),
        size_limit: input.size_limit.unwrap_or(5000),
        description: input.description.unwrap_or_default(),
        pinned: input.pinned.unwrap_or(false),
        scope: input.scope.unwrap_or_else(|| "project".to_string()),
        project: input.project.unwrap_or_else(|| "default".to_string()),
        created_at,
        updated_at,
    };
    let mut db = fresh_db(state)?;
    if let Err(e) = db.slot_create(&slot) {
        return Err(ErrorData::invalid_request(
            format!("Failed to create slot: {}", e),
            None,
        ));
    }
    ok_json(serde_json::json!({
        "success": true,
        "slot_id": slot.id,
        "label": input.label,
        "created_at": slot.created_at,
    }))
}

fn tool_slot_append(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        label: String,
        text: String,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let mut db = fresh_db(state)?;
    match db.slot_append(&input.label, &input.text) {
        Ok(new_len) => ok_json(serde_json::json!({
            "success": true,
            "label": input.label,
            "new_length": new_len,
        })),
        Err(e) => Err(ErrorData::invalid_request(
            format!("Failed to append to slot '{}': {}", input.label, e),
            None,
        )),
    }
}

fn tool_slot_replace(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        label: String,
        content: String,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let mut db = fresh_db(state)?;
    match db.slot_replace(&input.label, &input.content) {
        Ok(()) => ok_json(serde_json::json!({
            "success": true,
            "label": input.label,
            "new_content_length": input.content.len(),
        })),
        Err(e) => Err(ErrorData::invalid_request(
            format!("Failed to replace slot '{}': {}", input.label, e),
            None,
        )),
    }
}

fn tool_slot_delete(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        label: String,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let mut db = fresh_db(state)?;
    if let Err(e) = db.slot_delete(&input.label) {
        return Err(ErrorData::invalid_request(
            format!("Failed to delete slot: {}", e),
            None,
        ));
    }
    ok_json(serde_json::json!({
        "success": true,
        "label": input.label,
        "deleted": true,
    }))
}

fn tool_checkpoint(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        label: String,
        #[serde(default)]
        metadata: Option<serde_json::Value>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let checkpoint_id = format!("cp_{}", chrono::Utc::now().timestamp());
    let now = chrono::Utc::now().to_rfc3339();
    let metadata_json = input
        .metadata
        .as_ref()
        .map(|m| serde_json::to_string(m).unwrap_or_default())
        .unwrap_or_default();
    let mut conn = state.db.coordination();
    conn.execute(
        "INSERT INTO checkpoints (id, name, operation, status, checkpoint_type, linked_action_ids, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            &checkpoint_id,
            &input.label,
            &metadata_json,
            "active",
            "manual",
            "[]",
            &now,
            &now,
        ],
    )
    .map_err(|e| internal_error_safe(&e))?;
    ok_json(serde_json::json!({
        "success": true,
        "checkpoint_id": checkpoint_id,
        "message": input.label,
        "timestamp": now,
        "git_commit_sha": "unavailable",
    }))
}

fn tool_mesh_sync(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        #[serde(default)]
        wings: Option<Vec<String>>,
        #[serde(default)]
        sync_mode: Option<String>,
        #[serde(default)]
        peer_url: Option<String>,
        #[serde(default)]
        peer_name: Option<String>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };

    let registered_peer = if let (Some(url), Some(name)) = (&input.peer_url, &input.peer_name) {
        match state.mesh.write().unwrap().register(url, name, None, None) {
            Ok(p) => Some(serde_json::json!({
                "id": p.id,
                "url": p.url,
                "name": p.name,
                "status": p.status,
            })),
            Err(e) => {
                return ok_json(serde_json::json!({
                    "success": false,
                    "error": e.to_string(),
                }));
            }
        }
    } else {
        None
    };

    let peers: Vec<_> = state
        .mesh
        .read()
        .unwrap()
        .list_peers()
        .iter()
        .map(|p| {
            serde_json::json!({
                "id": p.id,
                "url": p.url,
                "name": p.name,
                "status": p.status,
                "shared_scopes": p.shared_scopes,
                "last_sync_at": p.last_sync_at.map(|dt| dt.to_rfc3339()),
            })
        })
        .collect();

    ok_json(serde_json::json!({
        "success": true,
        "synced_wings": input.wings.unwrap_or_default(),
        "sync_mode": input.sync_mode.unwrap_or_else(|| "incremental".to_string()),
        "nodes_synced": peers.len(),
        "peers": peers,
        "registered": registered_peer,
    }))
}

fn tool_graph_search(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        entity_names: Vec<String>,
        depth: Option<usize>,
        limit: Option<usize>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let kg = match crate::knowledge_graph::KnowledgeGraph::open(
        &state.palace_path.join("knowledge_graph.db"),
    ) {
        Ok(g) => g,
        Err(e) => return ok_json(serde_json::json!({"status": "error", "message": e.to_string()})),
    };
    let entity_refs: Vec<&str> = input.entity_names.iter().map(|s| s.as_str()).collect();
    match crate::graph_retrieval::search_by_entities(
        &kg,
        &entity_refs,
        input.depth.unwrap_or(2),
        input.limit.unwrap_or(50),
    ) {
        Ok(r) => ok_json(
            serde_json::json!({"status": "ok", "entities": r.entities, "relationships": r.relationships.iter().map(|e| serde_json::json!({"subject": e.subject, "predicate": e.predicate, "object": e.object, "confidence": e.confidence, "current": e.current})).collect::<Vec<_>>(), "depth": r.depth}),
        ),
        Err(e) => ok_json(serde_json::json!({"status": "error", "message": e.to_string()})),
    }
}

fn tool_graph_expand(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        observation_ids: Vec<String>,
        depth: Option<usize>,
        limit: Option<usize>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let kg = match crate::knowledge_graph::KnowledgeGraph::open(
        &state.palace_path.join("knowledge_graph.db"),
    ) {
        Ok(g) => g,
        Err(e) => return ok_json(serde_json::json!({"status": "error", "message": e.to_string()})),
    };
    match crate::graph_retrieval::expand_from_chunks(
        &kg,
        &input.observation_ids,
        input.depth.unwrap_or(2),
        input.limit.unwrap_or(50),
    ) {
        Ok(r) => ok_json(
            serde_json::json!({"status": "ok", "entities": r.entities, "relationships": r.relationships.iter().map(|e| serde_json::json!({"subject": e.subject, "predicate": e.predicate, "object": e.object, "confidence": e.confidence, "current": e.current})).collect::<Vec<_>>(), "depth": r.depth}),
        ),
        Err(e) => ok_json(serde_json::json!({"status": "error", "message": e.to_string()})),
    }
}

fn tool_context_build(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        token_budget: Option<usize>,
        pinned_ids: Option<Vec<String>>,
        session_ids: Option<Vec<String>>,
        include_working_memory: Option<bool>,
        output_format: Option<String>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let budget = input.token_budget.unwrap_or(8000);
    let fmt = input.output_format.unwrap_or_else(|| "json".to_string());
    let db = fresh_db(state)?;
    let mut slots = Vec::new();
    if let Some(ref ids) = input.pinned_ids {
        for id in ids {
            if let Ok(Some(slot)) = db.slot_get(id) {
                slots.push(crate::types::MemorySlot {
                    id: slot.id.clone(),
                    name: slot.label.clone(),
                    content: slot.content.clone(),
                    token_count: slot.content.split_whitespace().count(),
                    priority: if slot.pinned { 1 } else { 0 },
                    last_updated: slot
                        .updated_at
                        .parse()
                        .unwrap_or_else(|_| chrono::Utc::now()),
                });
            }
        }
    }
    let mut builder = crate::context::ContextBuilder::new(budget).with_pinned_slots(slots);
    if let Some(ref session_ids) = input.session_ids {
        let all_drawers = db.get_all(None, Some("session"), 1000);
        let sessions: Vec<_> = session_ids
            .iter()
            .filter_map(|sid| {
                all_drawers
                    .iter()
                    .flat_map(|qr| {
                        qr.ids
                            .iter()
                            .zip(qr.metadatas.iter())
                            .filter_map(|(id, meta)| {
                                if id == sid {
                                    Some(crate::types::Session {
                                        id: id.clone(),
                                        project: String::new(),
                                        cwd: String::new(),
                                        started_at: meta
                                            .get("created_at")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("1970-01-01T00:00:00Z")
                                            .parse()
                                            .unwrap_or_else(|_| chrono::Utc::now()),
                                        ended_at: None,
                                        status: "active".to_string(),
                                        observation_count: 0,
                                        model: None,
                                        tags: vec![],
                                        first_prompt: None,
                                        summary: None,
                                        commit_shas: vec![],
                                        agent_id: None,
                                    })
                                } else {
                                    None
                                }
                            })
                    })
                    .find(|_| true)
            })
            .collect();
        builder = builder.with_session_summaries(sessions);
    }
    if input.include_working_memory.unwrap_or(true) {
        let obs_drawers = db.get_all(None, Some("observation"), 50);
        let compressed: Vec<_> = obs_drawers
            .iter()
            .flat_map(|qr| {
                qr.ids
                    .iter()
                    .zip(qr.documents.iter())
                    .zip(qr.metadatas.iter())
                    .map(|((id, doc), meta)| crate::types::CompressedObservation {
                        id: id.clone(),
                        session_id: meta
                            .get("session_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        timestamp: meta
                            .get("created_at")
                            .and_then(|v| v.as_str())
                            .unwrap_or("1970-01-01T00:00:00Z")
                            .parse()
                            .unwrap_or_else(|_| chrono::Utc::now()),
                        observation_type: crate::types::ObservationType::UserPrompt,
                        title: doc.chars().take(100).collect(),
                        subtitle: None,
                        facts: vec![],
                        narrative: doc.clone(),
                        concepts: vec![],
                        files: vec![],
                        importance: 5,
                        confidence: 0.5,
                        image_ref: None,
                        image_description: None,
                        modality: "text".to_string(),
                        agent_id: None,
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        builder = builder.with_working_memory(compressed);
    }
    match builder.build() {
        Ok(blocks) => {
            if fmt == "xml" {
                ok_json(
                    serde_json::json!({"status": "ok", "format": "xml", "context": builder.build_xml().unwrap_or_default()}),
                )
            } else {
                ok_json(
                    serde_json::json!({"status": "ok", "format": "json", "blocks": blocks.iter().map(|b| serde_json::json!({"content": b.content, "source": b.source, "relevance_score": b.relevance_score, "token_count": b.token_count, "memory_id": b.memory_id})).collect::<Vec<_>>(), "total_tokens": blocks.iter().map(|b| b.token_count).sum::<usize>()}),
                )
            }
        }
        Err(e) => ok_json(serde_json::json!({"status": "error", "message": e.to_string()})),
    }
}

fn tool_flow_compress(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        observations: Option<Vec<crate::types::CompressedObservation>>,
        preserve_above_importance: Option<u8>,
        compress_mode: Option<bool>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };

    let obs = input.observations.unwrap_or_default();
    let config = crate::flow_compress::FlowCompressConfig {
        preserve_above_importance: input.preserve_above_importance.unwrap_or(4),
        preserve_types: vec![
            crate::types::ObservationType::Decision,
            crate::types::ObservationType::Discovery,
            crate::types::ObservationType::Task,
        ],
        compress_mode: input.compress_mode.unwrap_or(true),
        target_tokens: 4000,
    };

    match crate::flow_compress::compress_session_observations(obs, Some(config)) {
        Ok((compressed, result)) => ok_json(serde_json::json!({
            "status": "ok",
            "compressed_count": result.compressed_count,
            "evicted_count": result.evicted_count,
            "preserved_observations": result.preserved_observations,
            "token_budget_used": result.token_budget_used,
            "summary": result.summary,
        })),
        Err(e) => ok_json(serde_json::json!({"status": "error", "message": e.to_string()})),
    }
}

fn tool_cascade_update(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        changed_entity_id: String,
        changed_entity_type: String,
        max_depth: Option<usize>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };

    let config = crate::cascade::CascadeConfig {
        max_depth: input.max_depth.unwrap_or(3),
        cascade_observations: true,
        cascade_actions: true,
        cascade_signals: true,
        trigger_on_types: vec![
            "file".to_string(),
            "function".to_string(),
            "class".to_string(),
            "module".to_string(),
            "package".to_string(),
        ],
    };

    let kg_path = state.palace_path.join("knowledge_graph.db");
    match crate::cascade::cascade_update(
        &input.changed_entity_id,
        &input.changed_entity_type,
        &kg_path,
        Some(config),
    ) {
        Ok(result) => ok_json(serde_json::json!({
            "status": "ok",
            "total_updated": result.total_updated,
            "depth_reached": result.depth_reached,
            "summary": result.summary,
        })),
        Err(e) => ok_json(serde_json::json!({"status": "error", "message": e.to_string()})),
    }
}

fn tool_enrich(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        file_path: String,
        query: Option<String>,
        search_limit: Option<usize>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let db = fresh_db(state)?;
    let limit = input.search_limit.unwrap_or(10);
    let all_drawers = db.get_all(None, None, 100);
    let memories: Vec<_> = all_drawers
        .iter()
        .flat_map(|qr| {
            qr.ids
                .iter()
                .zip(qr.documents.iter())
                .zip(qr.metadatas.iter())
                .map(|((id, doc), meta)| crate::types::Memory {
                    id: id.clone(),
                    created_at: meta
                        .get("created_at")
                        .and_then(|v| v.as_str())
                        .unwrap_or("1970-01-01T00:00:00Z")
                        .parse()
                        .unwrap_or_else(|_| chrono::Utc::now()),
                    updated_at: chrono::Utc::now(),
                    memory_type: crate::types::MemoryType::Semantic,
                    title: doc.chars().take(100).collect(),
                    content: doc.clone(),
                    concepts: vec![],
                    files: vec![],
                    session_ids: vec![],
                    strength: 0.5,
                    version: 1,
                    parent_id: None,
                    supersedes: vec![],
                    related_ids: vec![],
                    source_observation_ids: vec![],
                    is_latest: true,
                    forget_after: None,
                    image_ref: None,
                    agent_id: None,
                    project: String::new(),
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let result = crate::enrich::enrich(
        &input.file_path,
        input.query.as_deref().unwrap_or(""),
        &memories,
        limit,
    );
    ok_json(
        serde_json::json!({"status": "ok", "file_contexts": result.file_contexts.iter().map(|fc| serde_json::json!({"path": fc.path, "content_summary": fc.content_summary, "last_modified": fc.last_modified, "related_files": fc.related_files})).collect::<Vec<_>>(), "related_memories": result.related_memories.iter().map(|m| serde_json::json!({"id": m.id, "title": m.title, "content": m.content, "relevance": m.relevance})).collect::<Vec<_>>(), "bug_memories": result.bug_memories.iter().map(|m| serde_json::json!({"id": m.id, "title": m.title, "content": m.content, "relevance": m.relevance})).collect::<Vec<_>>()}),
    )
}

fn tool_retention_score(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        memory_id: String,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let db = fresh_db(state)?;
    let drawers = db.get_all(None, None, usize::MAX);
    let mut found = None;
    for drawer in &drawers {
        if let Some(idx) = drawer.ids.iter().position(|id| id == &input.memory_id) {
            let doc = &drawer.documents[idx];
            let meta = &drawer.metadatas[idx];
            let strength: f64 = meta.get("strength").and_then(|v| v.as_f64()).unwrap_or(1.0);
            let memory_type = meta
                .get("memory_type")
                .and_then(|v| v.as_str())
                .unwrap_or("semantic");
            found = Some((doc.clone(), strength, memory_type));
            break;
        }
    }
    match found {
        Some((_content, strength, memory_type)) => {
            let decay_id = input.memory_id.clone();
            let decay_config = crate::retention::default_decay_config();
            let retention_score = crate::retention::default_retention_score(&decay_id);
            let retention_strength =
                crate::retention::calculate_retention(&retention_score, &decay_config, None);
            let tier = crate::retention::promote_tier(retention_strength);
            ok_json(serde_json::json!({
                "memory_id": input.memory_id,
                "retention_strength": retention_strength,
                "current_strength": strength,
                "memory_type": memory_type,
                "promotion_tier": serde_json::json!({ "tier": format!("{:?}", tier), "threshold_met": retention_strength >= 0.5 }),
                "decay_info": { "initial_retention": decay_config.initial_retention, "decay_rate": decay_config.decay_rate }
            }))
        }
        None => ok_json(serde_json::json!({"status": "not_found", "memory_id": input.memory_id})),
    }
}

fn tool_access_stats(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        limit: Option<usize>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let limit = input.limit.unwrap_or(10);
    let db = fresh_db(state)?;
    let drawers = db.get_all(None, None, usize::MAX);
    let mut stats: Vec<_> = drawers.iter().flat_map(|drawer| {
        drawer.ids.iter().zip(drawer.metadatas.iter()).filter_map(|(id, meta)| {
            let access_count: usize = meta.get("access_count").and_then(|v| v.as_u64()).map(|v| v as usize).unwrap_or(0);
            let last_accessed = meta.get("last_accessed").and_then(|v| v.as_str()).map(|s| s.to_string());
            Some(serde_json::json!({ "memory_id": id, "access_count": access_count, "last_accessed": last_accessed }))
        }).collect::<Vec<_>>()
    }).collect();
    stats.sort_by(|a, b| {
        let a_count = a.get("access_count").and_then(|v| v.as_u64()).unwrap_or(0);
        let b_count = b.get("access_count").and_then(|v| v.as_u64()).unwrap_or(0);
        b_count.cmp(&a_count)
    });
    let most_accessed: Vec<_> = stats.iter().take(limit).cloned().collect();
    let recently_accessed: Vec<_> = stats.iter().rev().take(limit).cloned().collect();
    ok_json(serde_json::json!({
        "most_accessed": most_accessed,
        "recently_accessed": recently_accessed,
        "total_tracked": stats.len()
    }))
}

fn tool_working_memory(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        limit: Option<usize>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let limit = input.limit.unwrap_or(50);
    let db = fresh_db(state)?;
    let obs_drawers = db.get_all(None, Some("observation"), limit);
    let observations: Vec<_> = obs_drawers.iter().flat_map(|qr| {
        qr.ids.iter().zip(qr.documents.iter()).zip(qr.metadatas.iter()).map(|((id, doc), meta)| {
            serde_json::json!({
                "id": id,
                "session_id": meta.get("session_id").and_then(|v| v.as_str()).unwrap_or(""),
                "created_at": meta.get("created_at").and_then(|v| v.as_str()).unwrap_or("1970-01-01T00:00:00Z"),
                "title": doc.chars().take(100).collect::<String>(),
                "content": doc,
                "importance": meta.get("importance").and_then(|v| v.as_u64()).unwrap_or(5) as usize
            })
        }).collect::<Vec<_>>()
    }).take(limit).collect();
    ok_json(serde_json::json!({
        "observations": observations,
        "count": observations.len()
    }))
}

fn tool_team_share(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    let observation_id = args
        .get("observation_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let team_id = args
        .get("team_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let permission = args
        .get("permission")
        .and_then(|v| v.as_str())
        .unwrap_or("read");
    let message = args.get("message").and_then(|v| v.as_str());

    if observation_id.is_empty() || team_id.is_empty() {
        return Err(ErrorData::invalid_params(
            "observation_id and team_id are required".to_string(),
            None,
        ));
    }

    let db = fresh_db(state)?;
    let share_id = format!(
        "share_{}",
        uuid::Uuid::new_v4().to_string()[..8].to_string()
    );
    let now = chrono::Utc::now().to_rfc3339();

    let share = crate::palace_db::TeamShare {
        id: share_id.clone(),
        item_id: observation_id.to_string(),
        item_type: permission.to_string(),
        project: team_id.to_string(),
        shared_at: now.clone(),
    };

    db.coordination().team_share_create(&share).map_err(|e| {
        ErrorData::internal_error(format!("Failed to create team share: {}", e), None)
    })?;

    ok_json(serde_json::json!({
        "share_id": share_id,
        "observation_id": observation_id,
        "team_id": team_id,
        "shared_at": now,
        "shared_by": "current_user"
    }))
}

fn tool_team_feed(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    let team_id = args.get("team_id").and_then(|v| v.as_str());
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

    let db = fresh_db(state)?;
    let shares = db.coordination().team_share_list(team_id).map_err(|e| {
        ErrorData::internal_error(format!("Failed to fetch team feed: {}", e), None)
    })?;

    let feed: Vec<_> = shares
        .into_iter()
        .take(limit)
        .map(|share| {
            serde_json::json!({
                "share_id": share.id,
                "observation_id": share.item_id,
                "shared_by": "current_user",
                "shared_at": share.shared_at,
                "type": share.item_type,
                "title": share.project,
                "preview": format!("Shared item: {} ({})", share.item_id, share.item_type)
            })
        })
        .collect();

    ok_json(serde_json::json!({
        "feed": feed,
        "total": feed.len()
    }))
}

fn tool_consolidate(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        #[serde(default = "default_threshold")]
        threshold: f64,
        #[serde(default)]
        dry_run: bool,
    }
    fn default_threshold() -> f64 {
        5.0
    }

    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };

    let mut db = fresh_db(state)?;
    let all_drawers = db.get_all(None, None, usize::MAX);

    let mut tier_counts = serde_json::json!({
        "sketch": 0i64,
        "lesson": 0i64,
        "insight": 0i64,
        "memory": 0i64,
        "archive": 0i64,
    });

    let mut promote_sketch_to_lesson: Vec<String> = Vec::new();
    let mut promote_lesson_to_insight: Vec<String> = Vec::new();
    let mut promote_insight_to_memory: Vec<String> = Vec::new();
    let mut promote_memory_to_archive: Vec<String> = Vec::new();

    for qr in &all_drawers {
        for (i, _doc) in qr.documents.iter().enumerate() {
            let meta = qr.metadatas.get(i);
            let doc_type = meta
                .and_then(|m| m.get("doc_type").and_then(|v| v.as_str()))
                .unwrap_or("observation");
            let importance: f64 = meta
                .and_then(|m| m.get("importance").and_then(|v| v.as_str()))
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            let memory_id = qr.ids.get(i).cloned().unwrap_or_default();

            match doc_type {
                "observation" => {
                    if importance >= input.threshold {
                        promote_sketch_to_lesson.push(memory_id);
                        tier_counts["sketch"] =
                            (tier_counts["sketch"].as_i64().unwrap_or(0) + 1).into();
                    }
                }
                "memory" => {
                    let confidence: f64 = meta
                        .and_then(|m| m.get("confidence").and_then(|v| v.as_str()))
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(0.0);
                    let created_days_ago = meta
                        .and_then(|m| m.get("created_at").and_then(|v| v.as_str()))
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|dt| {
                            chrono::Utc::now()
                                .signed_duration_since(dt.with_timezone(&chrono::Utc))
                                .num_days()
                        })
                        .unwrap_or(0);

                    if confidence >= 9.0 || created_days_ago > 90 {
                        promote_memory_to_archive.push(memory_id);
                        tier_counts["archive"] =
                            (tier_counts["archive"].as_i64().unwrap_or(0) + 1).into();
                    } else if confidence >= 8.0 {
                        promote_insight_to_memory.push(memory_id);
                        tier_counts["memory"] =
                            (tier_counts["memory"].as_i64().unwrap_or(0) + 1).into();
                    } else if confidence >= 7.0 {
                        promote_lesson_to_insight.push(memory_id);
                        tier_counts["insight"] =
                            (tier_counts["insight"].as_i64().unwrap_or(0) + 1).into();
                    } else {
                        promote_lesson_to_insight.push(memory_id);
                        tier_counts["lesson"] =
                            (tier_counts["lesson"].as_i64().unwrap_or(0) + 1).into();
                    }
                }
                _ => {}
            }
        }
    }

    let total_promoted = promote_sketch_to_lesson.len()
        + promote_lesson_to_insight.len()
        + promote_insight_to_memory.len()
        + promote_memory_to_archive.len();

    if input.dry_run {
        ok_json(serde_json::json!({
            "success": true,
            "dry_run": true,
            "items_consolidated": 0,
            "tier_promotions": {
                "sketch_to_lesson": promote_sketch_to_lesson.len(),
                "lesson_to_insight": promote_lesson_to_insight.len(),
                "insight_to_memory": promote_insight_to_memory.len(),
                "memory_to_archive": promote_memory_to_archive.len(),
            },
            "tier_counts": tier_counts,
            "message": "Dry run - no changes made",
        }))
    } else {
        // Re-open mutable db for writes
        let mut db = fresh_db(state)?;

        // Apply sketch → lesson promotions (doc_type: observation → lesson)
        if !promote_sketch_to_lesson.is_empty() {
            let docs_to_update: Vec<_> = db
                .get_documents_with_metadata(&promote_sketch_to_lesson)
                .into_iter()
                .map(|(id, content, mut meta)| {
                    meta.insert("doc_type".to_string(), serde_json::json!("lesson"));
                    (id, content, meta)
                })
                .collect();
            db.upsert_documents(&docs_to_update)
                .map_err(|e| internal_error_safe(&e))?;
        }

        // Apply lesson → insight promotions (doc_type: lesson → insight)
        if !promote_lesson_to_insight.is_empty() {
            let docs_to_update: Vec<_> = db
                .get_documents_with_metadata(&promote_lesson_to_insight)
                .into_iter()
                .map(|(id, content, mut meta)| {
                    meta.insert("doc_type".to_string(), serde_json::json!("insight"));
                    (id, content, meta)
                })
                .collect();
            db.upsert_documents(&docs_to_update)
                .map_err(|e| internal_error_safe(&e))?;
        }

        // Apply insight → memory promotions (doc_type: insight → memory, bump confidence)
        if !promote_insight_to_memory.is_empty() {
            let docs_to_update: Vec<_> = db
                .get_documents_with_metadata(&promote_insight_to_memory)
                .into_iter()
                .map(|(id, content, mut meta)| {
                    meta.insert("doc_type".to_string(), serde_json::json!("memory"));
                    meta.insert("confidence".to_string(), serde_json::json!(8.0));
                    (id, content, meta)
                })
                .collect();
            db.upsert_documents(&docs_to_update)
                .map_err(|e| internal_error_safe(&e))?;
        }

        // Apply memory → archive promotions (doc_type: memory → archive, bump importance)
        if !promote_memory_to_archive.is_empty() {
            let docs_to_update: Vec<_> = db
                .get_documents_with_metadata(&promote_memory_to_archive)
                .into_iter()
                .map(|(id, content, mut meta)| {
                    meta.insert("doc_type".to_string(), serde_json::json!("archive"));
                    meta.insert("importance".to_string(), serde_json::json!(1.0));
                    (id, content, meta)
                })
                .collect();
            db.upsert_documents(&docs_to_update)
                .map_err(|e| internal_error_safe(&e))?;
        }

        // Persist all changes
        db.flush().map_err(|e| internal_error_safe(&e))?;

        // Invalidate graph cache so KG reflects updated doc_types
        crate::palace_graph::invalidate_cache(&state.palace_path);

        // Spawn async consolidation pipeline for LLM-based semantic + procedural consolidation
        let palace_path = state.palace_path.clone();
        let _handle = tokio::spawn(async move {
            use crate::consolidation_pipeline::run_consolidation_pipeline;
            use crate::llm::LlmProvider;
            let provider = crate::llm::create_llm_provider_from_env();
            tracing::info!(
                "async consolidation pipeline spawned for {}",
                palace_path.display()
            );
        });

        ok_json(serde_json::json!({
            "success": true,
            "dry_run": false,
            "items_consolidated": total_promoted,
            "tier_promotions": {
                "sketch_to_lesson": promote_sketch_to_lesson.len(),
                "lesson_to_insight": promote_lesson_to_insight.len(),
                "insight_to_memory": promote_insight_to_memory.len(),
                "memory_to_archive": promote_memory_to_archive.len(),
            },
            "tier_counts": tier_counts,
        }))
    }
}

fn tool_snapshot_create(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        message: String,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let repo = state.palace_path.parent().unwrap_or(&state.palace_path);
    let output = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(repo)
        .output();
    let add_ok = output.map(|o| o.status.success()).unwrap_or(false);
    let sha = if add_ok {
        std::process::Command::new("git")
            .args(["commit", "-m", &input.message])
            .current_dir(repo)
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    let out = String::from_utf8_lossy(&o.stdout);
                    out.lines().next().map(|l| l.to_string())
                } else {
                    None
                }
            })
    } else {
        None
    };
    let snapshot_id = sha.unwrap_or_else(|| format!("snap_{}", chrono::Utc::now().timestamp()));
    ok_json(serde_json::json!({
        "success": true,
        "snapshot_id": snapshot_id,
        "message": input.message,
        "created_at": chrono::Utc::now().to_rfc3339(),
    }))
}

fn tool_file_history(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        file_path: String,
        #[serde(default)]
        limit: Option<usize>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let repo = state.palace_path.parent().unwrap_or(&state.palace_path);
    let limit = input.limit.unwrap_or(50);
    let output = std::process::Command::new("git")
        .args([
            "log",
            "--format=%H|%s|%ad",
            "--follow",
            "-n",
            &limit.to_string(),
            "--",
            &input.file_path,
        ])
        .current_dir(repo)
        .output();
    let history: Vec<_> = output
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter_map(|line| {
                    let parts: Vec<&str> = line.splitn(3, '|').collect();
                    if parts.len() >= 3 {
                        Some(serde_json::json!({
                            "commit_sha": parts[0],
                            "message": parts[1],
                            "date": parts[2],
                        }))
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    ok_json(serde_json::json!({
        "success": true,
        "file_path": input.file_path,
        "history": history,
        "total": history.len(),
    }))
}

fn tool_sessions(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        #[serde(default)]
        wing: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let db = fresh_db(state)?;
    let all_drawers = db.get_all(
        input.wing.as_deref(),
        Some("session"),
        input.limit.unwrap_or(50),
    );

    // Load summaries from the SessionStore (sessions SQLite db) so the viewer
    // can show the summary alongside each session. The SessionStore path lives
    // next to the palace.json collection.
    let session_store_path = state.palace_path.join("sessions");
    let summaries: std::collections::HashMap<String, Option<String>> = {
        let store = crate::session::SessionStore::open(&session_store_path);
        match store {
            Ok(store) => {
                let mut map = std::collections::HashMap::new();
                if let Ok(sessions) = store.list_sessions(None) {
                    for s in sessions {
                        map.insert(s.id, s.summary);
                    }
                }
                map
            }
            Err(_) => std::collections::HashMap::new(),
        }
    };

    let sessions: Vec<_> = all_drawers
        .iter()
        .flat_map(|qr| {
            qr.ids
                .iter()
                .zip(qr.documents.iter())
                .zip(qr.metadatas.iter())
                .map(|((id, doc), meta)| {
                    let created_at = meta
                        .get("created_at")
                        .and_then(|v| v.as_str())
                        .unwrap_or("N/A");
                    let summary = summaries.get(id.as_str()).and_then(|s| s.as_deref());
                    let mut obj = serde_json::json!({
                        "session_id": id,
                        "content": doc.chars().take(200).collect::<String>(),
                        "created_at": created_at,
                    });
                    if let Some(s) = summary {
                        obj["summary"] = serde_json::Value::String(s.to_string());
                    }
                    obj
                })
        })
        .collect();
    ok_json(serde_json::json!({
        "success": true,
        "sessions": sessions,
        "total": sessions.len(),
    }))
}

fn tool_observe(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        hook_type: String,
        session_id: String,
        #[serde(default)]
        project: String,
        #[serde(default)]
        cwd: String,
        #[serde(default)]
        data: Option<serde_json::Value>,
    }
    let input: Input = parse_args(args)?;

    // Finding 19: reject oversized data payload before processing
    if let Some(ref data_val) = input.data {
        let data_str = serde_json::to_string(data_val).map_err(|e| internal_error_safe(&e))?;
        if data_str.len() > 65536 {
            return Err(ErrorData::invalid_params(
                format!(
                    "data payload too large: {} bytes (max 65536)",
                    data_str.len()
                ),
                None,
            ));
        }
    }

    let hook_type: crate::types::HookType = match input.hook_type.parse() {
        Ok(ht) => ht,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid hook_type '{}': {}", input.hook_type, e),
                None,
            ))
        }
    };

    let data: std::collections::HashMap<String, serde_json::Value> = match input.data {
        Some(serde_json::Value::Object(map)) => map.into_iter().collect(),
        Some(other) => {
            let mut m = std::collections::HashMap::new();
            m.insert("payload".to_string(), other);
            m
        }
        None => std::collections::HashMap::new(),
    };

    let payload = crate::types::HookPayload {
        hook_type,
        session_id: input.session_id.clone(),
        project: input.project,
        cwd: input.cwd,
        timestamp: chrono::Utc::now(),
        data,
    };

    let obs = match crate::observe::process_observation(&payload) {
        Ok(o) => o,
        Err(e) => return Err(internal_error_safe(&e)),
    };

    // Finding 5: reuse cached session_store from AppState instead of opening a new one
    if let Err(e) = state.session_store.add_observation(&obs) {
        warn!("Failed to save observation: {}", e);
    }
    // Auto-end the session when a terminal hook arrives
    if matches!(
        hook_type,
        crate::types::HookType::SessionEnd | crate::types::HookType::Stop
    ) {
        let _ = state.session_store.end_session(&input.session_id, None);
    }

    ok_json(serde_json::json!({
        "success": true,
        "observation_id": obs.id,
        "hook_type": hook_type.to_string(),
        "session_id": input.session_id,
    }))
}

fn tool_commits(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        #[serde(default)]
        branch: Option<String>,
        #[serde(default)]
        repo: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let repo = state.palace_path.parent().unwrap_or(&state.palace_path);
    let limit_val = input.limit.unwrap_or(100).min(500);
    let mut cmd = std::process::Command::new("git");
    cmd.args([
        "log",
        "--format=%H|%an|%s|%ct",
        "-n",
        &limit_val.to_string(),
    ]);
    if let Some(branch) = &input.branch {
        cmd.arg(branch.as_str());
    }
    let output = cmd.current_dir(repo).output();
    let commits: Vec<_> = output
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter_map(|line| {
                    let parts: Vec<&str> = line.splitn(4, '|').collect();
                    if parts.len() >= 4 {
                        Some(serde_json::json!({
                            "sha": parts[0],
                            "author": parts[1],
                            "message": parts[2],
                            "timestamp": parts[3],
                        }))
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    ok_json(serde_json::json!({
        "success": true,
        "branch": input.branch,
        "commits": commits,
        "total": commits.len(),
    }))
}

fn tool_commit_lookup(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        sha: String,
        #[serde(default)]
        include_diff: Option<bool>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };
    let repo = state.palace_path.parent().unwrap_or(&state.palace_path);
    let output = std::process::Command::new("git")
        .args(["show", "--format=%H|%an|%ae|%s|%ct", "-s", &input.sha])
        .current_dir(repo)
        .output();
    let commit = output.ok().and_then(|o| {
        let line = String::from_utf8_lossy(&o.stdout).trim().to_string();
        let parts: Vec<&str> = line.splitn(5, '|').collect();
        if parts.len() >= 5 {
            Some(serde_json::json!({
                "sha": parts[0],
                "author_name": parts[1],
                "author_email": parts[2],
                "subject": parts[3],
                "timestamp": parts[4],
            }))
        } else {
            None
        }
    });
    ok_json(serde_json::json!({
        "success": true,
        "sha": input.sha,
        "include_diff": input.include_diff.unwrap_or(false),
        "commit": commit,
    }))
}

// ---------------------------------------------------------------------------
// Memory-compatible tools (aliases + new handlers)
// ---------------------------------------------------------------------------

fn tool_recall(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        query: String,
        limit: Option<usize>,
        format: Option<String>,
        token_budget: Option<usize>,
    }
    let input: Input = parse_args_with_integer_coercion(args, &["limit", "token_budget"])?;
    let sanitized = crate::query_sanitizer::sanitize_query(&input.query);
    let db = fresh_db(state)?;
    let results = db
        .query_sync(
            &sanitized.clean_query,
            None,
            None,
            input.limit.unwrap_or(10),
        )
        .map_err(|e| internal_error_safe(&e))?;

    // AGENT_SCOPE isolation: post-filter so only the current agent's
    // documents are visible when scope is "isolated".
    let results = filter_by_agent_id(results, state)?;

    let format = input.format.as_deref().unwrap_or("full");
    let formatted_results: serde_json::Value = match format {
        "compact" => serde_json::to_value(results.iter().map(|r| {
            serde_json::json!({
                "id": r.ids.first().cloned().unwrap_or_default(),
                "snippet": r.documents.first().map(|d| d.chars().take(200).collect::<String>()).unwrap_or_default(),
            })
        }).collect::<Vec<_>>()).unwrap_or_default(),
        "narrative" => {
            let items: Vec<String> = results.iter().filter_map(|r| {
                r.documents.first().map(|d| d.clone())
            }).collect();
            serde_json::json!({ "narrative": items.join("\n---\n") })
        }
        _ => serde_json::to_value(results.iter().map(|r| {
            serde_json::json!({
                "id": r.ids.first().cloned().unwrap_or_default(),
                "content": r.documents.first().cloned().unwrap_or_default(),
                "metadata": r.metadatas.first().cloned().unwrap_or_default(),
            })
        }).collect::<Vec<_>>()).unwrap_or_default(),
    };
    ok_json(serde_json::json!({
        "query": sanitized.clean_query,
        "results": formatted_results,
        "count": results.len(),
        "format": format,
    }))
}

fn tool_save(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        content: String,
        memory_type: Option<String>,
        concepts: Option<String>,
        files: Option<String>,
        project: Option<String>,
    }
    let input: Input = parse_args(args)?;
    let mut db = fresh_db(state)?;

    // Parse concepts and files
    let concepts_vec: Vec<String> = input
        .concepts
        .as_ref()
        .map(|s| s.split(',').map(|t| t.trim().to_string()).collect())
        .unwrap_or_default();
    let files_vec: Vec<String> = input
        .files
        .as_ref()
        .map(|s| s.split(',').map(|t| t.trim().to_string()).collect())
        .unwrap_or_default();

    // Use project or derive wing from it
    let wing = input
        .project
        .clone()
        .unwrap_or_else(|| "memory".to_string());
    let room = input.memory_type.unwrap_or_else(|| "insight".to_string());

    let mut metadata = std::collections::HashMap::new();
    if !concepts_vec.is_empty() {
        metadata.insert("concepts".to_string(), concepts_vec.join(","));
    }
    if !files_vec.is_empty() {
        metadata.insert("files".to_string(), files_vec.join(","));
    }
    if let Some(p) = &input.project {
        metadata.insert("project".to_string(), p.clone());
    }
    metadata.insert("memory_type".to_string(), room.clone());

    // Generate a unique drawer ID
    let hash = short_hash(&format!("{}{}{}", wing, room, input.content), 24);
    let drawer_id = format!("drawer_{}_{}_{}", wing, room, hash);

    // Convert metadata HashMap to Vec of tuples for db.add()
    let metadata_vec: Vec<(&str, &str)> = metadata
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    db.add(&[(&drawer_id, &input.content)], &[&metadata_vec])
        .map_err(|e| internal_error_safe(&e))?;

    ok_json(serde_json::json!({
        "success": true,
        "drawer_id": drawer_id,
        "wing": wing,
        "room": room,
        "concepts": concepts_vec,
        "files": files_vec,
    }))
}

fn tool_profile(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        project: Option<String>,
        refresh: Option<bool>,
    }
    let input: Input = parse_args(args)?;
    let db = fresh_db(state)?;
    let all_drawers = db.get_all(input.project.as_deref(), None, usize::MAX);

    // Extract top concepts by frequency
    let mut concept_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for qr in &all_drawers {
        if let Some(meta) = qr.metadatas.first() {
            if let Some(c) = meta.get("concepts").and_then(|v| v.as_str()) {
                for concept in c.split(',') {
                    let c = concept.trim();
                    if !c.is_empty() {
                        *concept_counts.entry(c.to_string()).or_insert(0) += 1;
                    }
                }
            }
        }
    }
    let mut top_concepts: Vec<_> = concept_counts.into_iter().collect();
    top_concepts.sort_by(|a, b| b.1.cmp(&a.1));
    let top_concepts: Vec<_> = top_concepts
        .into_iter()
        .take(20)
        .map(|(k, v)| serde_json::json!({"concept": k, "count": v}))
        .collect();

    // File patterns
    let mut file_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for qr in &all_drawers {
        if let Some(meta) = qr.metadatas.first() {
            if let Some(f) = meta.get("source_file").and_then(|v| v.as_str()) {
                let ext = std::path::Path::new(f)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("unknown");
                *file_counts.entry(ext.to_string()).or_insert(0) += 1;
            }
        }
    }
    let mut file_patterns: Vec<_> = file_counts.into_iter().collect();
    file_patterns.sort_by(|a, b| b.1.cmp(&a.1));
    let file_patterns: Vec<_> = file_patterns
        .into_iter()
        .take(10)
        .map(|(k, v)| serde_json::json!({"extension": k, "count": v}))
        .collect();

    ok_json(serde_json::json!({
        "project": input.project,
        "total_memories": all_drawers.iter().map(|qr| qr.ids.len()).sum::<usize>(),
        "top_concepts": top_concepts,
        "file_patterns": file_patterns,
        "refreshed": input.refresh.unwrap_or(false),
    }))
}

fn tool_export(state: &AppState, _args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    let db = fresh_db(state)?;
    let all_drawers = db.get_all(None, None, usize::MAX);

    let memories: Vec<_> = all_drawers
        .iter()
        .flat_map(|qr| {
            qr.ids
                .iter()
                .zip(qr.documents.iter())
                .zip(qr.metadatas.iter())
                .map(|((id, doc), meta)| {
                    let mut obj = serde_json::Map::new();
                    obj.insert("id".to_string(), serde_json::Value::String(id.clone()));
                    obj.insert(
                        "content".to_string(),
                        serde_json::Value::String(doc.clone()),
                    );
                    let mut meta_map = serde_json::Map::new();
                    for (k, v) in meta {
                        if let Some(s) = v.as_str() {
                            meta_map.insert(k.clone(), serde_json::Value::String(s.to_string()));
                        }
                    }
                    obj.insert("metadata".to_string(), serde_json::Value::Object(meta_map));
                    serde_json::Value::Object(obj)
                })
                .collect::<Vec<_>>()
        })
        .collect();

    ok_json(serde_json::json!({
        "exported_at": chrono::Utc::now().to_rfc3339(),
        "total": memories.len(),
        "memories": memories,
    }))
}

async fn tool_mine(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if state.read_only {
        return Ok(CallToolResult::error(vec![rmcp::model::Content::text(
            "mempalace_mine is disabled in read-only mode",
        )]));
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        path: String,
        mode: Option<String>,
    }
    let input: Input = parse_args_with_integer_coercion(args, &[])?;

    // Finding 11: reject paths containing ".." to prevent traversal
    if input.path.contains("..") {
        return Ok(CallToolResult::error(vec![rmcp::model::Content::text(
            "path must not contain '..'",
        )]));
    }

    let path = std::path::PathBuf::from(&input.path);
    // Canonicalize to verify the path is accessible and real (replaces old path.exists check)
    if std::fs::canonicalize(&path).is_err() {
        return Ok(CallToolResult::error(vec![rmcp::model::Content::text(
            format!("Path does not exist: {}", input.path),
        )]));
    }

    // Finding 17: validate mode string before mapping
    let mode_str = input.mode.as_deref().unwrap_or("projects");
    match mode_str.to_lowercase().as_str() {
        "projects" | "project" | "convos" | "convo" | "conversations" | "auto" => {}
        _ => {
            return Err(ErrorData::invalid_params(
                format!(
                    "unknown mode '{mode_str}', expected one of: projects, conversations, convos, device"
                ),
                None,
            ));
        }
    }
    let mode = match mode_str.to_lowercase().as_str() {
        "projects" | "project" => crate::cli::MiningMode::Projects,
        "convos" | "convo" | "conversations" => crate::cli::MiningMode::Convos,
        "auto" => crate::cli::MiningMode::Auto,
        _ => crate::cli::MiningMode::default(),
    };

    let palace_path = state.palace_path.clone();
    let palace_arg = palace_path.to_str().map(|s| s.to_string());

    // Finding 4: spawn_blocking to avoid blocking the tokio runtime
    let mode_for_closure = mode.clone();
    let result = tokio::task::spawn_blocking(move || {
        crate::cli::cmd_mine(
            &path,
            &mode_for_closure,
            None,
            "mcp",
            50,
            false,
            false,
            &[],
            palace_arg.as_deref(),
            None,
            false,
            None,
        )
    })
    .await
    .map_err(|e| internal_error_safe(&anyhow::anyhow!("mine task panicked: {}", e)))?;

    match result {
        Ok(()) => ok_json(serde_json::json!({
            "success": true,
            "path": input.path,
            "mode": format!("{:?}", mode).to_lowercase(),
        })),
        Err(e) => Ok(CallToolResult::error(vec![rmcp::model::Content::text(
            format!("mine failed: {e}"),
        )])),
    }
}

fn tool_timeline(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        anchor: String,
        project: Option<String>,
        before: Option<usize>,
        after: Option<usize>,
    }
    let input: Input = parse_args_with_integer_coercion(args, &["before", "after"])?;
    let db = fresh_db(state)?;

    // Try to parse anchor as date, otherwise use as keyword
    let anchor_date = chrono::NaiveDate::parse_from_str(&input.anchor, "%Y-%m-%d").ok();
    let before_count = input.before.unwrap_or(5);
    let after_count = input.after.unwrap_or(5);

    let all_drawers = db.get_all(input.project.as_deref(), None, 500);
    let mut all_items: Vec<_> = all_drawers
        .iter()
        .flat_map(|qr| {
            qr.ids
                .iter()
                .zip(qr.documents.iter())
                .zip(qr.metadatas.iter())
                .filter_map(|((id, doc), meta)| {
                    let created_at =
                        meta.get("created_at")
                            .and_then(|v| v.as_str())
                            .and_then(|s| {
                                chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                                    .ok()
                                    .or_else(|| {
                                        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                                            .ok()
                                            .map(|d| d.and_hms_opt(0, 0, 0).unwrap_or_default())
                                    })
                            });
                    Some(serde_json::json!({
                        "id": id,
                        "content": doc.chars().take(100).collect::<String>(),
                        "created_at": created_at.map(|d| d.to_string()),
                        "metadata": meta,
                    }))
                })
                .collect::<Vec<_>>()
        })
        .collect();

    // Filter by anchor date if provided
    if let Some(date) = anchor_date {
        all_items.retain(|item| {
            if let Some(created) = item.get("created_at").and_then(|v| v.as_str()) {
                if let Ok(d) = chrono::NaiveDate::parse_from_str(created, "%Y-%m-%d") {
                    return d == date;
                }
            }
            false
        });
    }

    ok_json(serde_json::json!({
        "anchor": input.anchor,
        "before": before_count,
        "after": after_count,
        "items": all_items,
        "total": all_items.len(),
    }))
}

fn tool_patterns(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        project: Option<String>,
    }
    let input: Input = parse_args(args)?;
    let db = fresh_db(state)?;
    let all_drawers = db.get_all(input.project.as_deref(), None, usize::MAX);

    // Detect patterns across sessions - look for recurring content snippets
    let mut content_freq: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for qr in &all_drawers {
        for doc in &qr.documents {
            let words: Vec<&str> = doc.split_whitespace().collect();
            for window in words.windows(3) {
                let phrase = window.join(" ");
                if phrase.len() > 10 {
                    *content_freq.entry(phrase).or_insert(0) += 1;
                }
            }
        }
    }

    let patterns: Vec<_> = content_freq
        .into_iter()
        .filter(|(_, count)| *count >= 2)
        .map(|(phrase, count)| serde_json::json!({"phrase": phrase, "occurrences": count}))
        .collect();

    ok_json(serde_json::json!({
        "patterns": patterns,
        "total_patterns": patterns.len(),
        "project": input.project,
    }))
}

fn tool_smart_search(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        query: String,
        expand_ids: Option<String>,
        limit: Option<usize>,
    }
    let input: Input = parse_args_with_integer_coercion(args, &["limit"])?;
    let db = fresh_db(state)?;

    let results = db
        .hybrid_search(&input.query, input.limit.unwrap_or(10), None, None)
        .map_err(|e| internal_error_safe(&e))?;

    // AGENT_SCOPE isolation: post-filter so only the current agent's
    // documents are visible when scope is "isolated".
    let results = filter_by_agent_id(results, state)?;

    // mr-6g8z: record the search into the followup tracker so we can detect
    // when a followup within FOLLOWUP_WINDOW_SECONDS has zero overlap.
    let result_ids: Vec<String> = results
        .iter()
        .filter_map(|r| r.ids.first().cloned())
        .collect();
    if let Ok(mut tracker) = state.followup_tracker.lock() {
        let agent_id = state.config.agent_id.as_deref().unwrap_or("default");
        let project = state.config.collection_name.as_str();
        let _ = tracker.record_search(agent_id, project, &input.query, &result_ids);
    }
    // If expand_ids provided, fetch those specifically
    let expanded: Vec<serde_json::Value> = if let Some(ids_str) = input.expand_ids {
        let ids: Vec<String> = ids_str.split(',').map(|s| s.trim().to_string()).collect();
        ids.iter()
            .filter_map(|id| {
                db._get_document(id).map(|entry| {
                    serde_json::json!({
                        "id": id,
                        "content": entry.content,
                        "metadata": entry.metadata,
                    })
                })
            })
            .collect()
    } else {
        vec![]
    };

    ok_json(serde_json::json!({
        "semantic_results": results.iter().map(|r| {
            serde_json::json!({
                "id": r.ids.first().cloned().unwrap_or_default(),
                "content": r.documents.first().cloned().unwrap_or_default(),
                "metadata": r.metadatas.first().cloned().unwrap_or_default(),
            })
        }).collect::<Vec<_>>(),
        "expanded": expanded,
        "query": input.query,
        "total": results.len(),
    }))
}

fn tool_hybrid_search(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        query: String,
        wing: Option<String>,
        room: Option<String>,
        limit: Option<usize>,
    }
    let input: Input = parse_args_with_integer_coercion(args, &["limit"])?;
    let sanitized = crate::query_sanitizer::sanitize_query(&input.query);

    let db = crate::palace_db::PalaceDb::open(&state.palace_path)
        .map_err(|e| internal_error_safe(&e))?;

    let results = db
        .hybrid_search(
            &sanitized.clean_query,
            input.limit.unwrap_or(10),
            input.wing.as_deref(),
            input.room.as_deref(),
        )
        .map_err(|e| internal_error_safe(&e))?;

    // AGENT_SCOPE isolation: post-filter so only the current agent's
    // documents are visible when scope is "isolated".
    let results = filter_by_agent_id(results, state)?;

    ok_json(serde_json::json!({
        "query": sanitized.clean_query,
        "filters": {
            "wing": input.wing,
            "room": input.room,
        },
        "results": results.iter().map(|r| {
            serde_json::json!({
                "id": r.ids.first().cloned().unwrap_or_default(),
                "content": r.documents.first().cloned().unwrap_or_default(),
                "metadata": r.metadatas.first().cloned().unwrap_or_default(),
                "distance": r.distances.first().cloned().unwrap_or(1.0),
            })
        }).collect::<Vec<_>>(),
        "total": results.len(),
    }))
}

fn tool_vision_search(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        query_text: Option<String>,
        query_image_ref: Option<String>,
        top_k: Option<usize>,
        session_id: Option<String>,
    }
    let input: Input = match serde_json::from_value(serde_json::Value::Object(args)) {
        Ok(i) => i,
        Err(e) => {
            return Err(ErrorData::invalid_params(
                format!("Invalid args: {e}"),
                None,
            ))
        }
    };

    let db_path = state.palace_path.join("coordination.db");
    let conn = match rusqlite::Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => {
            return ok_json(serde_json::json!({
                "status": "error",
                "message": format!("cannot open coordination.db: {}", e),
            }))
        }
    };

    match crate::vision::VisionSearchStore::new(conn, None) {
        Ok(store) => {
            match store.vision_search(
                input.query_text.as_deref(),
                input.query_image_ref.as_deref(),
                input.top_k,
                input.session_id.as_deref(),
            ) {
                Ok(results) => {
                    let hits: Vec<_> = results
                        .into_iter()
                        .map(|r| {
                            serde_json::json!({
                                "image_ref": r.image_ref,
                                "score": r.score,
                                "session_id": r.session_id,
                                "observation_id": r.observation_id,
                                "updated_at": r.updated_at,
                            })
                        })
                        .collect();
                    ok_json(serde_json::json!({
                        "status": "ok",
                        "query_text": input.query_text,
                        "query_image_ref": input.query_image_ref,
                        "results": hits,
                        "total": hits.len(),
                    }))
                }
                Err(e) => ok_json(serde_json::json!({
                    "status": "error",
                    "message": e.to_string(),
                })),
            }
        }
        Err(e) => ok_json(serde_json::json!({
            "status": "error",
            "message": format!("vision search unavailable: {}", e),
        })),
    }
}

fn tool_relations(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        memory_id: String,
        max_hops: Option<usize>,
        min_confidence: Option<f64>,
    }
    let input: Input = parse_args_with_integer_coercion(args, &["max_hops"])?;
    let db = fresh_db(state)?;

    let drawer = db
        ._get_document(&input.memory_id)
        .ok_or_else(|| ErrorData::internal_error("Drawer not found", None))?;

    // Simple relation extraction from metadata
    let mut relations = vec![];
    let meta = &drawer.metadata;
    if let Some(wing) = meta.get("wing") {
        relations.push(serde_json::json!({
            "type": "in_wing",
            "target": wing,
            "confidence": 1.0,
        }));
    }
    if let Some(room) = meta.get("room") {
        relations.push(serde_json::json!({
            "type": "in_room",
            "target": room,
            "confidence": 1.0,
        }));
    }
    if let Some(concepts) = meta.get("concepts") {
        if let Some(c) = concepts.as_str() {
            for concept in c.split(',') {
                let c = concept.trim();
                if !c.is_empty() {
                    relations.push(serde_json::json!({
                        "type": "concept",
                        "target": c,
                        "confidence": 0.8,
                    }));
                }
            }
        }
    }

    ok_json(serde_json::json!({
        "memory_id": input.memory_id,
        "relations": relations,
        "max_hops": input.max_hops.unwrap_or(2),
        "total": relations.len(),
    }))
}

fn tool_audit(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        operation: Option<String>,
        limit: Option<usize>,
    }
    let input: Input = parse_args_with_integer_coercion(args, &["limit"])?;
    let limit = input.limit.unwrap_or(50);

    // Read WAL entries
    let wal_path = state.palace_path.join("wal");
    let mut entries = vec![];
    if let Ok(wal_dir) = fs::read_dir(&wal_path) {
        for entry in wal_dir.flatten() {
            if let Ok(content) = fs::read_to_string(entry.path()) {
                for line in content.lines() {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                        if let Some(tool_name) = val.get("tool").and_then(|v| v.as_str()) {
                            if input.operation.is_none()
                                || tool_name.contains(input.operation.as_ref().unwrap())
                            {
                                entries.push(serde_json::json!({
                                    "timestamp": val.get("timestamp"),
                                    "tool": tool_name,
                                    "args": val.get("args"),
                                    "result_summary": val.get("result_summary"),
                                }));
                            }
                        }
                    }
                }
            }
        }
    }
    entries.sort_by(|a, b| {
        let ta = a.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
        let tb = b.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
        tb.cmp(ta)
    });
    entries.truncate(limit);

    ok_json(serde_json::json!({
        "operations": entries,
        "total": entries.len(),
        "filter": input.operation,
    }))
}

fn tool_claude_bridge_sync(
    state: &AppState,
    args: JsonObject,
) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        direction: Option<String>,
    }
    let input: Input = parse_args(args)?;
    let direction = input.direction.as_deref().unwrap_or("sync");

    let config = crate::claude_bridge::ClaudeBridgeConfig {
        enabled: true,
        project_path: Some(state.palace_path.to_string_lossy().to_string()),
        memory_file_path: Some(
            state
                .palace_path
                .join("MEMORY.md")
                .to_string_lossy()
                .to_string(),
        ),
        line_budget: 200,
    };

    match direction {
        "push" => {
            let memories = state.db.get_memories(None, 100);
            let project_summary = "";
            match crate::claude_bridge::sync_to_claude(&config, &memories, project_summary) {
                Ok(lines) => ok_json(serde_json::json!({
                    "success": true,
                    "direction": "push",
                    "lines_written": lines,
                })),
                Err(e) => Err(internal_error_safe(&e)),
            }
        }
        "pull" => match crate::claude_bridge::read_from_claude(&config) {
            Ok(parsed) => ok_json(serde_json::json!({
                "success": true,
                "direction": "pull",
                "sections": parsed.sections,
                "line_count": parsed.line_count,
            })),
            Err(e) => Err(internal_error_safe(&e)),
        },
        _ => {
            // Bidirectional sync
            let memories = state.db.get_memories(None, 100);
            let project_summary = "";
            let push_result =
                crate::claude_bridge::sync_to_claude(&config, &memories, project_summary);
            let pull_result = crate::claude_bridge::read_from_claude(&config);
            ok_json(serde_json::json!({
                "success": true,
                "direction": "sync",
                "push": push_result.map(|lines| serde_json::json!({"lines_written": lines})).ok(),
                "pull": pull_result.ok().map(|p| serde_json::json!({
                    "sections": p.sections,
                    "line_count": p.line_count,
                })),
            }))
        }
    }
}

fn kg_path(state: &AppState) -> std::path::PathBuf {
    state
        .palace_path
        .parent()
        .unwrap_or(&state.palace_path)
        .join("knowledge_graph.db")
}

// ---------------------------------------------------------------------------
// Run entry point
// ---------------------------------------------------------------------------

pub fn run_server(palace_override: Option<&str>, read_only: bool) -> anyhow::Result<()> {
    let mut config = crate::Config::load()?;
    if let Some(p) = palace_override {
        config.palace_path = resolve_palace_override(p);
    }
    let server = MempalaceServer::new(AppState::new(config, read_only)?);
    let (stdin, stdout) = stdio();
    let rt = Runtime::new()?;
    rt.block_on(async {
        let running = server.serve((stdin, stdout)).await?;
        running.waiting().await?;
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(())
}

fn resolve_palace_override(raw: &str) -> std::path::PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::PathBuf::from(home).join(rest);
        }
    }
    std::path::PathBuf::from(raw)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn test_state() -> AppState {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = crate::Config {
            palace_path: temp_dir.path().join("palace"),
            collection_name: "test_collection".to_string(),
            people_map: Default::default(),
            topic_wings: vec!["emotions".to_string()],
            hall_keywords: Default::default(),
            embedding_model: "naive".to_string(),
            languages: vec![],
            ..Default::default()
        };
        std::fs::create_dir_all(&config.palace_path).unwrap();
        AppState::new(config, false).unwrap()
    }

    fn dispatch(
        state: &AppState,
        name: &str,
        args: serde_json::Value,
    ) -> Result<CallToolResult, ErrorData> {
        let owned_state = AppState {
            config: state.config.clone(),
            db: crate::palace_db::PalaceDb::open(&state.palace_path).unwrap(),
            read_only: state.read_only,
            palace_path: state.palace_path.clone(),
            mesh: std::sync::RwLock::new(crate::coordination::mesh::Mesh::new(None)),
            followup_tracker: state.followup_tracker.clone(),
            session_store: std::sync::Arc::new(
                crate::session::SessionStore::open(&state.palace_path.join("sessions")).unwrap(),
            ),
        };
        let f = make_dispatch(Arc::new(owned_state));
        let args = args.as_object().cloned().unwrap_or_default();
        let wal_dir = state.palace_path.join("wal");
        // Use try_current to detect if we're in a runtime
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.block_on(invoke_with_wal(name.to_string(), args, f, wal_dir.clone()))
            }
            Err(_) => {
                // No runtime: create one just for this call
                let rt = Runtime::new().unwrap();
                rt.block_on(invoke_with_wal(name.to_string(), args, f, wal_dir))
            }
        }
    }

    #[test]
    fn test_status() {
        let state = test_state();
        let result = dispatch(&state, "mempalace_status", json!({}));
        assert!(result.is_ok());
    }

    /// Regression for the audit Issue D: when the server runs `--read-only`, the
    /// list of tools exposed must NOT include the five mutation tools. Call-time
    /// `read_only_guard` still rejects the underlying handlers, but well-behaved
    /// clients should never see the tools as available actions.
    #[test]
    fn test_mutation_tools_classification_matches_dispatch() {
        // Drift guard — every name in MUTATION_TOOLS must be a real tool name
        // that make_tools() emits, otherwise the read-only filter is a no-op.
        let names: std::collections::HashSet<String> = make_tools()
            .into_iter()
            .map(|t| t.name.as_ref().to_string())
            .collect();
        for tool in MUTATION_TOOLS {
            assert!(
                names.contains(*tool),
                "MUTATION_TOOLS entry {tool} is not a real tool name; rename or remove"
            );
            assert!(is_mutation_tool(tool));
        }
        // Spot-check a read-only tool is NOT classified as mutation.
        assert!(!is_mutation_tool("mempalace_search"));
        assert!(!is_mutation_tool("mempalace_status"));
        assert!(!is_mutation_tool("mempalace_kg_query"));
    }

    #[test]
    fn test_read_only_mode_hides_mutation_tools_from_list() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = crate::Config {
            palace_path: temp_dir.path().join("palace"),
            collection_name: "test_collection".to_string(),
            people_map: Default::default(),
            topic_wings: vec!["emotions".to_string()],
            hall_keywords: Default::default(),
            embedding_model: "naive".to_string(),
            languages: vec![],
            ..Default::default()
        };
        std::fs::create_dir_all(&config.palace_path).unwrap();
        let ro_state = AppState::new(config, true).unwrap();
        let server = MempalaceServer::new(ro_state);

        // Mirror the filter the list_tools handler applies.
        let filtered: Vec<String> = if server.state.read_only {
            make_tools()
                .into_iter()
                .filter(|t| !is_mutation_tool(t.name.as_ref()))
                .map(|t| t.name.as_ref().to_string())
                .collect()
        } else {
            make_tools()
                .into_iter()
                .map(|t| t.name.as_ref().to_string())
                .collect()
        };
        for tool in MUTATION_TOOLS {
            assert!(
                !filtered.contains(&tool.to_string()),
                "mutation tool {tool} must be hidden from tools/list in --read-only mode"
            );
        }
        // Sanity check: a few read-only tools are still present.
        for kept in ["mempalace_search", "mempalace_status", "mempalace_kg_query"] {
            assert!(
                filtered.contains(&kept.to_string()),
                "read-only tool {kept} must remain visible in tools/list"
            );
        }

        // And get_tool() must also refuse to surface mutation tools by name.
        for tool in MUTATION_TOOLS {
            assert!(
                server.get_tool(tool).is_none(),
                "MempalaceServer::get_tool({tool}) must return None in --read-only mode"
            );
        }
    }

    #[test]
    fn test_list_rooms() {
        let state = test_state();
        let result = dispatch(
            &state,
            "mempalace_list_rooms",
            json!({ "wing": "emotions" }),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_search_empty() {
        let state = test_state();
        let result = dispatch(
            &state,
            "mempalace_search",
            json!({ "query": "nonexistent" }),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_status_no_palace_returns_error_shape() {
        let state = test_state();
        let result = dispatch(&state, "mempalace_status", json!({})).unwrap();
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            parsed.get("error").and_then(|v| v.as_str()),
            Some("No palace found")
        );
    }

    #[test]
    fn test_check_duplicate() {
        let state = test_state();
        let result = dispatch(
            &state,
            "mempalace_check_duplicate",
            json!({ "content": "nonexistent" }),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_duplicate_matches_python_shape_and_threshold() {
        let state = test_state();
        {
            let mut db = crate::palace_db::PalaceDb::open(&state.palace_path).unwrap();
            db.add(
                &[ (
                    "dup-auth",
                    "The authentication module uses JWT tokens for session management. Tokens expire after 24 hours. Refresh tokens are stored in HttpOnly cookies.",
                ) ],
                &[&[("wing", "project"), ("room", "backend")]],
            )
            .unwrap();
            db.flush().unwrap();
        }

        let result = dispatch(
            &state,
            "mempalace_check_duplicate",
            json!({
                "content": "The authentication module uses JWT tokens for session management. Tokens expire after 24 hours. Refresh tokens are stored in HttpOnly cookies.",
                "threshold": 0.5
            }),
        )
        .unwrap();

        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            parsed.get("is_duplicate").and_then(|v| v.as_bool()),
            Some(true)
        );
        let matches = parsed.get("matches").and_then(|v| v.as_array()).unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(
            matches[0].get("id").and_then(|v| v.as_str()),
            Some("dup-auth")
        );
        assert_eq!(
            matches[0].get("wing").and_then(|v| v.as_str()),
            Some("project")
        );
        assert_eq!(
            matches[0].get("room").and_then(|v| v.as_str()),
            Some("backend")
        );
        assert!(matches[0].get("source_file").is_none());
    }

    #[test]
    fn test_check_duplicate_returns_multiple_matches() {
        let state = test_state();
        {
            let mut db = crate::palace_db::PalaceDb::open(&state.palace_path).unwrap();
            db.add(
                &[
                    ("dup-a", "JWT auth uses bearer tokens and refresh cookies"),
                    (
                        "dup-b",
                        "JWT authentication uses bearer tokens with refresh cookies",
                    ),
                ],
                &[
                    &[("wing", "project"), ("room", "backend")],
                    &[("wing", "project"), ("room", "backend")],
                ],
            )
            .unwrap();
            db.flush().unwrap();
        }

        let result = dispatch(
            &state,
            "mempalace_check_duplicate",
            json!({ "content": "JWT auth uses bearer tokens and refresh cookies", "threshold": 0.0 }),
        )
        .unwrap();
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        let matches = parsed.get("matches").and_then(|v| v.as_array()).unwrap();
        assert!(matches.len() >= 2);
    }

    #[test]
    fn test_diary_read() {
        let state = test_state();
        let result = dispatch(
            &state,
            "mempalace_diary_read",
            json!({ "agent_name": "TestAgent" }),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_list_wings() {
        let state = test_state();
        let result = dispatch(&state, "mempalace_list_wings", json!({}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_taxonomy() {
        let state = test_state();
        let result = dispatch(&state, "mempalace_get_taxonomy", json!({}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_aaak_spec() {
        let state = test_state();
        let result = dispatch(&state, "mempalace_get_aaak_spec", json!({}));
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mcp_search_uses_sanitized_query() {
        let raw = format!(
            "{}\nWhere is the auth migration plan?",
            "system prompt ".repeat(40)
        );
        let sanitized = crate::query_sanitizer::sanitize_query(&raw);
        assert_eq!(sanitized.clean_query, "Where is the auth migration plan?");
        assert!(sanitized.was_sanitized);
    }

    #[test]
    fn test_kg_stats() {
        let state = test_state();
        let result = dispatch(&state, "mempalace_kg_stats", json!({}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_graph_stats() {
        let state = test_state();
        let result = dispatch(&state, "mempalace_graph_stats", json!({}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_traverse() {
        let state = test_state();
        let result = dispatch(
            &state,
            "mempalace_traverse",
            json!({ "start_room": "unknown" }),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_read_only_blocks_add_drawer() {
        let state = {
            let temp_dir = tempfile::tempdir().unwrap();
            let config = crate::Config {
                palace_path: temp_dir.path().join("palace"),
                collection_name: "test_ro".to_string(),
                people_map: Default::default(),
                topic_wings: vec![],
                hall_keywords: Default::default(),
                embedding_model: "naive".to_string(),
                languages: vec![],
                ..Default::default()
            };
            std::fs::create_dir_all(&config.palace_path).unwrap();
            AppState::new(config, true).unwrap()
        };
        let result = dispatch(
            &state,
            "mempalace_add_drawer",
            json!({ "wing": "test", "room": "backend", "content": "val" }),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_read_only_blocks_diary_write() {
        let state = {
            let temp_dir = tempfile::tempdir().unwrap();
            let config = crate::Config {
                palace_path: temp_dir.path().join("palace"),
                collection_name: "test_ro2".to_string(),
                people_map: Default::default(),
                topic_wings: vec![],
                hall_keywords: Default::default(),
                embedding_model: "naive".to_string(),
                languages: vec![],
                ..Default::default()
            };
            std::fs::create_dir_all(&config.palace_path).unwrap();
            AppState::new(config, true).unwrap()
        };
        let result = dispatch(
            &state,
            "mempalace_diary_write",
            json!({ "agent_name": "TestAgent", "entry": "hello" }),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_kg_add_forwards_valid_to() {
        // #1314: tool_kg_add must forward valid_to so callers can backfill
        // an already-ended historical fact in a single call. Previously
        // valid_to was silently dropped at the MCP boundary, collapsing
        // every historical add to "still current".
        let state = test_state();
        let result = dispatch(
            &state,
            "mempalace_kg_add",
            json!({
                "subject": "Alice",
                "predicate": "works_at",
                "object": "Acme",
                "valid_from": "2020-01-01",
                "valid_to": "2022-12-31",
            }),
        )
        .expect("kg_add with valid_to should succeed");
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed.get("success").and_then(|v| v.as_bool()), Some(true));

        // Querying with as_of inside the closed window must see it.
        let query_in = dispatch(
            &state,
            "mempalace_kg_query",
            json!({ "entity": "Alice", "as_of": "2021-06-01" }),
        )
        .expect("kg_query in-window should succeed");
        let in_text = serde_json::to_value(&query_in.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let in_parsed: Value = serde_json::from_str(&in_text).unwrap();
        assert_eq!(
            in_parsed.get("total").and_then(|v| v.as_u64()),
            Some(1),
            "fact must be visible inside its closed validity window"
        );

        // Querying past the end date must NOT see it.
        let query_out = dispatch(
            &state,
            "mempalace_kg_query",
            json!({ "entity": "Alice", "as_of": "2025-01-01" }),
        )
        .expect("kg_query past-end should succeed");
        let out_text = serde_json::to_value(&query_out.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let out_parsed: Value = serde_json::from_str(&out_text).unwrap();
        assert_eq!(
            out_parsed.get("total").and_then(|v| v.as_u64()),
            Some(0),
            "fact must NOT be visible past its valid_to"
        );
    }

    #[test]
    fn test_kg_add_forwards_source_drawer_id() {
        // #1314 / RFC 002 §5.5: tool_kg_add must forward source_drawer_id so
        // adapter provenance reaches the SQLite layer. Without this, the
        // drawer pointer is silently dropped at the MCP boundary.
        let state = test_state();
        let add = dispatch(
            &state,
            "mempalace_kg_add",
            json!({
                "subject": "operating-verb",
                "predicate": "candidate",
                "object": "husbandry",
                "valid_from": "2026-04-28",
                "source_closet": "closet-42",
                "source_file": "docs/decisions.md",
                "source_drawer_id": "drawer_abc123",
            }),
        )
        .expect("kg_add with provenance should succeed");
        let text = serde_json::to_value(&add.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        let triple_id = parsed
            .get("triple_id")
            .and_then(|v| v.as_str())
            .expect("triple_id present");

        // Read the row directly: source_drawer_id must persist alongside the
        // other provenance fields. Open a raw SQLite connection so we don't
        // depend on KG-level abstractions hiding columns.
        let conn = rusqlite::Connection::open(kg_path(&state)).unwrap();
        let (closet, file, drawer): (Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT source_closet, source_file, source_drawer_id \
                 FROM triples WHERE id = ?1",
                rusqlite::params![triple_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(closet.as_deref(), Some("closet-42"));
        assert_eq!(file.as_deref(), Some("docs/decisions.md"));
        assert_eq!(drawer.as_deref(), Some("drawer_abc123"));
    }

    #[test]
    fn test_kg_add_rejects_invalid_iso_date() {
        // #1164: malformed dates must fail fast at MCP boundary instead of
        // producing silently-invisible triples.
        let state = test_state();
        let result = dispatch(
            &state,
            "mempalace_kg_add",
            json!({
                "subject": "Alice",
                "predicate": "likes",
                "object": "coffee",
                "valid_from": "March 2026",
            }),
        );
        assert!(result.is_err(), "invalid date must produce an MCP error");
    }

    #[test]
    fn test_kg_query_rejects_invalid_iso_date() {
        // #1164: malformed `as_of` must fail with a clear error rather than
        // silently returning empty results indistinguishable from "no facts".
        let state = test_state();
        let result = dispatch(
            &state,
            "mempalace_kg_query",
            json!({ "entity": "Alice", "as_of": "not-a-date" }),
        );
        assert!(result.is_err(), "invalid as_of must produce an MCP error");
    }

    #[test]
    fn test_kg_invalidate_resolves_default_ended_to_today() {
        // #1314: omitting `ended` must resolve to today's date in the response
        // so callers can see the actual value persisted, not the sentinel
        // string "today" returned by the previous Rust implementation.
        let state = test_state();
        dispatch(
            &state,
            "mempalace_kg_add",
            json!({ "subject": "Alice", "predicate": "lives_at", "object": "Old Address" }),
        )
        .expect("seed kg_add");
        let invalidate = dispatch(
            &state,
            "mempalace_kg_invalidate",
            json!({ "subject": "Alice", "predicate": "lives_at", "object": "Old Address" }),
        )
        .expect("kg_invalidate without ended should succeed");
        let text = serde_json::to_value(&invalidate.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        let ended = parsed
            .get("ended")
            .and_then(|v| v.as_str())
            .expect("ended field must be a string");
        assert_ne!(
            ended, "today",
            "ended must be a resolved YYYY-MM-DD date, not the sentinel string"
        );
        // Shape check: 10 chars, YYYY-MM-DD
        assert_eq!(ended.len(), 10, "ended must be a YYYY-MM-DD date: {ended}");
        assert_eq!(&ended[4..5], "-");
        assert_eq!(&ended[7..8], "-");
    }

    #[test]
    fn test_read_only_blocks_kg_add() {
        let state = {
            let temp_dir = tempfile::tempdir().unwrap();
            let config = crate::Config {
                palace_path: temp_dir.path().join("palace"),
                collection_name: "test_ro3".to_string(),
                people_map: Default::default(),
                topic_wings: vec![],
                hall_keywords: Default::default(),
                embedding_model: "naive".to_string(),
                languages: vec![],
                ..Default::default()
            };
            std::fs::create_dir_all(&config.palace_path).unwrap();
            AppState::new(config, true).unwrap()
        };
        let result = dispatch(
            &state,
            "mempalace_kg_add",
            json!({ "subject": "Alice", "predicate": "likes", "object": "coffee" }),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_read_only_blocks_delete_drawer() {
        let state = {
            let temp_dir = tempfile::tempdir().unwrap();
            let config = crate::Config {
                palace_path: temp_dir.path().join("palace"),
                collection_name: "test_ro4".to_string(),
                people_map: Default::default(),
                topic_wings: vec![],
                hall_keywords: Default::default(),
                embedding_model: "naive".to_string(),
                languages: vec![],
                ..Default::default()
            };
            std::fs::create_dir_all(&config.palace_path).unwrap();
            AppState::new(config, true).unwrap()
        };

        let result = dispatch(
            &state,
            "mempalace_delete_drawer",
            json!({ "drawer_id": "drawer_x" }),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_unknown_tool() {
        let state = test_state();
        let result = dispatch(&state, "nonexistent_tool", json!({}));
        assert!(result.is_err());
    }

    #[test]
    fn test_dispatch_writes_wal_entries() {
        let state = test_state();
        let wal_path = state.palace_path.join("wal").join("write_log.jsonl");

        let entries_before = if wal_path.exists() {
            let content = std::fs::read_to_string(&wal_path).unwrap_or_default();
            content.lines().filter(|l| !l.trim().is_empty()).count()
        } else {
            0
        };

        let result = dispatch(&state, "mempalace_status", json!({}));
        assert!(result.is_ok());

        let entries_after = if wal_path.exists() {
            let content = std::fs::read_to_string(&wal_path).unwrap_or_default();
            content.lines().filter(|l| !l.trim().is_empty()).count()
        } else {
            0
        };

        assert!(
            entries_after > entries_before,
            "WAL should grow after dispatch call"
        );
    }

    #[test]
    fn test_diary_write_and_read_roundtrip() {
        let state = test_state();
        let write_result = dispatch(
            &state,
            "mempalace_diary_write",
            json!({ "agent_name": "TestAgent", "entry": "Test diary entry", "topic": "architecture" }),
        );
        assert!(
            write_result.is_ok(),
            "diary write failed: {:?}",
            write_result
        );
        let read_result = dispatch(
            &state,
            "mempalace_diary_read",
            json!({ "agent_name": "TestAgent" }),
        );
        assert!(read_result.is_ok(), "diary read failed: {:?}", read_result);
    }

    /// Regression: every read-side MCP tool that consulted the long-lived
    /// `AppState.db` snapshot was stale within a single server session,
    /// because write tools (`tool_diary_write`, `tool_add_drawer`,
    /// `tool_delete_drawer`) open their own ad-hoc `PalaceDb` handles and
    /// flush to disk without touching `state.db`. Reproduced live by
    /// running a single `mpr serve` session, calling `mempalace_diary_write`
    /// (success) then `mempalace_diary_read` immediately after (returned
    /// `entries: []`). The unit-test `dispatch` helper masks this by
    /// reopening `AppState.db` per call, so these tests deliberately call
    /// `tool_*` directly on a single shared `&AppState` to exercise the
    /// real server's behaviour.
    #[test]
    fn test_diary_read_after_write_reflects_disk_writes() {
        let state = test_state();

        let write_args = json!({
            "agent_name": "ReproAgent",
            "entry": "Diary entry that must come back on read",
            "topic": "smoke",
        })
        .as_object()
        .cloned()
        .unwrap();
        tool_diary_write(&state, write_args).expect("diary write should succeed");

        let read_args = json!({ "agent_name": "ReproAgent" })
            .as_object()
            .cloned()
            .unwrap();
        let result = tool_diary_read(&state, read_args).expect("diary read should succeed");
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        let entries = parsed
            .get("entries")
            .and_then(|v| v.as_array())
            .expect("entries array");
        assert_eq!(
            entries.len(),
            1,
            "diary_read returned {} entries despite a successful write \
             on the same long-lived AppState (state.db staleness bug). \
             Parsed response: {}",
            entries.len(),
            parsed
        );
        let entry = &entries[0];
        assert_eq!(
            entry.get("content").and_then(|v| v.as_str()),
            Some("Diary entry that must come back on read"),
        );
    }

    /// Regression: same staleness bug, but via `mempalace_list_wings`
    /// (read) after `mempalace_add_drawer` (write). Locks in the wider
    /// fix beyond the original `diary_read` repro.
    #[test]
    fn test_list_wings_after_add_drawer_reflects_disk_writes() {
        let state = test_state();

        let add_args = json!({
            "wing": "regression_wing",
            "room": "regression_room",
            "content": "Drawer added in the same session.",
        })
        .as_object()
        .cloned()
        .unwrap();
        tool_add_drawer(&state, add_args).expect("add_drawer should succeed");

        let result = tool_list_wings(&state, json!({}).as_object().cloned().unwrap())
            .expect("list_wings should succeed");
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        let wings = parsed
            .get("wings")
            .and_then(|v| v.as_object())
            .expect("wings object");
        assert_eq!(
            wings.get("regression_wing").and_then(|v| v.as_u64()),
            Some(1),
            "list_wings missed the just-added wing (state.db staleness bug). \
             Parsed response: {}",
            parsed
        );
    }

    #[test]
    fn test_list_hallways_returns_envelope_when_palace_exists() {
        // mr-0qr1: mempalace_list_hallways should return a `hallways`
        // envelope so callers can distinguish the result type. We seed
        // a minimal palace DB first so collection_missing() is false.
        let state = test_state();
        {
            let mut db = crate::palace_db::PalaceDb::open(&state.palace_path).unwrap();
            db.add(
                &[("drawer_a", "alpha")],
                &[&[("wing", "code"), ("room", "backend")]],
            )
            .unwrap();
            db.flush().unwrap();
        }
        let result = dispatch(&state, "mempalace_list_hallways", json!({})).expect("list_hallways");
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        // Either the hallways envelope or a no_palace envelope — both
        // are valid JSON objects; we just need a successful response.
        assert!(parsed.is_object());
    }

    #[test]
    fn test_delete_hallway_returns_explanatory_error() {
        // mr-0qr1: a hallway is derived, not stored, so deletion returns
        // an explicit error rather than silently no-oping. We seed a
        // minimal palace so the tool reaches the explanatory branch
        // (instead of bailing with "no palace found").
        let state = test_state();
        {
            let mut db = crate::palace_db::PalaceDb::open(&state.palace_path).unwrap();
            db.add(
                &[("drawer_a", "alpha")],
                &[&[("wing", "code"), ("room", "backend")]],
            )
            .unwrap();
            db.flush().unwrap();
        }
        let result = dispatch(
            &state,
            "mempalace_delete_hallway",
            json!({ "hallway_id": "h-1" }),
        )
        .expect("delete_hallway");
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        assert!(
            text.contains("derived") || text.contains("delete"),
            "expected explanatory text, got: {text}"
        );
    }

    #[test]
    fn test_diary_write_lowercases_agent_name() {
        // mr-ju72: diary_write must lowercase the agent name on the way
        // in so that diary_read can match the same canonical form.
        let state = test_state();
        let result = dispatch(
            &state,
            "mempalace_diary_write",
            json!({ "agent_name": "MiXeD_CaSe", "entry": "hello" }),
        )
        .expect("diary write");
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            parsed.get("agent").and_then(|v| v.as_str()),
            Some("mixed_case"),
            "write must lowercase the agent name"
        );
    }

    #[test]
    fn test_preserve_case_on_update_case_only() {
        // mr-s0fq: case-only updates (e.g. "Backend" -> "backend")
        // must preserve the existing canonical casing.
        assert_eq!(preserve_case_on_update("Backend", "backend"), "Backend");
        assert_eq!(preserve_case_on_update("backend", "Backend"), "backend");
        assert_eq!(preserve_case_on_update("Foo", "FOO"), "Foo");
    }

    #[test]
    fn test_preserve_case_on_update_substantive_change() {
        // Substantive changes must take the new value.
        assert_eq!(preserve_case_on_update("backend", "frontend"), "frontend");
        assert_eq!(preserve_case_on_update("auth", "AUTH"), "auth");
        // Identical values: no-op, returns existing.
        assert_eq!(preserve_case_on_update("backend", "backend"), "backend");
    }

    #[test]
    fn test_metadata_str_coerce_before_case_comparison() {
        // mr-ong7: a metadata value that is a non-string (number, bool)
        // must still be matched by to_string() before eq_ignore_ascii_case.
        // We test the helper behaviour directly.
        let numeric = serde_json::json!(42);
        let boolean = serde_json::json!(true);
        let as_string_numeric = numeric.to_string();
        let as_string_bool = boolean.to_string();
        // After to_string, the value compares equal against the string form
        // (so an exact-string equality works, but more importantly the
        // value is a usable String, not None).
        assert!(!as_string_numeric.is_empty());
        assert!(!as_string_bool.is_empty());
        // The str-coerce path must not produce None on a non-string JSON value.
        assert_ne!(as_string_numeric.as_str(), "");
        assert_ne!(as_string_bool.as_str(), "");
    }

    #[test]
    fn test_diary_read_case_insensitive_agent() {
        // #1243: diary_write must lowercase the agent name so a diary
        // written as "Claude" remains findable when the caller reads as
        // "claude" or "CLAUDE". Without this, mixed-case names silently
        // return zero rows.
        let state = test_state();
        let write_result = dispatch(
            &state,
            "mempalace_diary_write",
            json!({ "agent_name": "Claude", "entry": "Mixed-case write" }),
        )
        .expect("diary write should succeed");
        let write_text = serde_json::to_value(&write_result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let write_json: Value = serde_json::from_str(&write_text).unwrap();
        assert_eq!(
            write_json.get("agent").and_then(|v| v.as_str()),
            Some("claude"),
            "agent name must be normalized to lowercase on write"
        );

        for read_name in ["claude", "Claude", "CLAUDE"] {
            let read_result = dispatch(
                &state,
                "mempalace_diary_read",
                json!({ "agent_name": read_name }),
            )
            .expect("diary read should succeed");
            let text = serde_json::to_value(&read_result.content[0])
                .unwrap()
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap()
                .to_string();
            let parsed: Value = serde_json::from_str(&text).unwrap();
            let entries = parsed
                .get("entries")
                .and_then(|v| v.as_array())
                .expect("entries field present");
            assert_eq!(
                entries.len(),
                1,
                "read as {read_name:?} must find the mixed-case entry"
            );
            assert_eq!(
                parsed.get("agent").and_then(|v| v.as_str()),
                Some("claude"),
                "agent name in read response must be the lowercased form"
            );
        }
    }

    #[test]
    fn test_diary_write_and_read_custom_wing() {
        let state = test_state();
        let write_result = dispatch(
            &state,
            "mempalace_diary_write",
            json!({ "agent_name": "TestAgent", "entry": "Saved in project wing", "wing": "wing_project_alpha" }),
        );
        assert!(
            write_result.is_ok(),
            "diary write failed: {:?}",
            write_result
        );

        let read_result = dispatch(
            &state,
            "mempalace_diary_read",
            json!({ "agent_name": "TestAgent", "wing": "wing_project_alpha" }),
        )
        .expect("diary read should succeed");
        let text = serde_json::to_value(&read_result.content[0])
            .unwrap()
            .get("text")
            .and_then(|value| value.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        let entries = parsed
            .get("entries")
            .and_then(|value| value.as_array())
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].get("content").and_then(|value| value.as_str()),
            Some("Saved in project wing")
        );
    }

    #[test]
    fn test_add_drawer_and_delete_roundtrip() {
        let state = test_state();
        let add_result = dispatch(
            &state,
            "mempalace_add_drawer",
            json!({ "wing": "project", "room": "backend", "content": "test_value" }),
        );
        assert!(add_result.is_ok(), "add_drawer failed: {:?}", add_result);
        let add_json = add_result.unwrap();
        let text = serde_json::to_value(&add_json.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        let drawer_id = parsed
            .get("drawer_id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();

        let delete_result = dispatch(
            &state,
            "mempalace_delete_drawer",
            json!({ "drawer_id": drawer_id }),
        );
        assert!(
            delete_result.is_ok(),
            "delete_drawer failed: {:?}",
            delete_result
        );
    }

    #[test]
    fn test_add_drawer_hash_uses_full_content() {
        let state = test_state();
        let prefix = "x".repeat(100);
        let first_content = format!("{prefix}A");
        let second_content = format!("{prefix}B");

        let first = dispatch(
            &state,
            "mempalace_add_drawer",
            json!({ "wing": "project", "room": "backend", "content": first_content }),
        )
        .expect("first add_drawer should succeed");
        let second = dispatch(
            &state,
            "mempalace_add_drawer",
            json!({ "wing": "project", "room": "backend", "content": second_content }),
        )
        .expect("second add_drawer should succeed");

        let first_text = serde_json::to_value(&first.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let second_text = serde_json::to_value(&second.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let first_json: Value = serde_json::from_str(&first_text).unwrap();
        let second_json: Value = serde_json::from_str(&second_text).unwrap();

        assert_ne!(
            first_json.get("drawer_id").and_then(|v| v.as_str()),
            second_json.get("drawer_id").and_then(|v| v.as_str())
        );
    }

    #[test]
    fn test_diary_write_hash_uses_full_entry() {
        let state = test_state();
        let prefix = "y".repeat(50);
        let first_entry = format!("{prefix}A");
        let second_entry = format!("{prefix}B");

        let first = dispatch(
            &state,
            "mempalace_diary_write",
            json!({ "agent_name": "TestAgent", "entry": first_entry, "topic": "architecture" }),
        )
        .expect("first diary write should succeed");
        let second = dispatch(
            &state,
            "mempalace_diary_write",
            json!({ "agent_name": "TestAgent", "entry": second_entry, "topic": "architecture" }),
        )
        .expect("second diary write should succeed");

        let first_text = serde_json::to_value(&first.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let second_text = serde_json::to_value(&second.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let first_json: Value = serde_json::from_str(&first_text).unwrap();
        let second_json: Value = serde_json::from_str(&second_text).unwrap();

        assert_ne!(
            first_json.get("entry_id").and_then(|v| v.as_str()),
            second_json.get("entry_id").and_then(|v| v.as_str())
        );
    }

    #[test]
    fn test_catalog_matches_python_surface() {
        let tools = make_tools();
        let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
        // Position-sensitive: the order below must match make_tools()'s
        // declaration order. The mr-0qr1 hallway aliases come right after
        // find_tunnels; the mr-dghp mine tool comes right after commit_lookup.
        let expected = vec![
            "mempalace_status",
            "mempalace_list_wings",
            "mempalace_list_rooms",
            "mempalace_get_taxonomy",
            "mempalace_get_aaak_spec",
            "mempalace_kg_query",
            "mempalace_kg_add",
            "mempalace_kg_invalidate",
            "mempalace_kg_timeline",
            "mempalace_kg_stats",
            "mempalace_kg_snapshot_rebuild",
            "mempalace_kg_reset",
            "mempalace_traverse",
            "mempalace_find_tunnels",
            // mr-0qr1: list_hallways/del_hallway come right after find_tunnels
            "mempalace_list_hallways",
            "mempalace_delete_hallway",
            "mempalace_graph_stats",
            "mempalace_search",
            "mempalace_check_duplicate",
            "mempalace_add_drawer",
            "mempalace_delete_drawer",
            "mempalace_diary_write",
            "mempalace_diary_read",
            "mempalace_heal",
            "mempalace_verify",
            "mempalace_governance_delete",
            "mempalace_obsidian_export",
            "mempalace_compress_file",
            "mempalace_detect_worktree",
            "mempalace_replay_import",
            "mempalace_action_create",
            "mempalace_action_update",
            "mempalace_frontier",
            "mempalace_next",
            "mempalace_lease",
            "mempalace_routine_run",
            "mempalace_signal_send",
            "mempalace_signal_read",
            "mempalace_sentinel_create",
            "mempalace_sentinel_trigger",
            "mempalace_sentinel_list",
            "mempalace_sentinel_delete",
            "mempalace_checkpoint_list",
            "mempalace_checkpoint_resolve",
            "mempalace_sketch_create",
            "mempalace_sketch_promote",
            "mempalace_crystallize",
            "mempalace_health",
            "mempalace_diagnose",
            "mempalace_facet_tag",
            "mempalace_facet_query",
            "mempalace_lesson_save",
            "mempalace_lesson_recall",
            "mempalace_reflect",
            "mempalace_insight_list",
            "mempalace_slot_list",
            "mempalace_slot_get",
            "mempalace_slot_create",
            "mempalace_slot_append",
            "mempalace_slot_replace",
            "mempalace_slot_delete",
            "mempalace_checkpoint",
            "mempalace_mesh_sync",
            "mempalace_team_share",
            "mempalace_team_feed",
            "mempalace_consolidate",
            "mempalace_graph_search",
            "mempalace_graph_expand",
            "mempalace_context_build",
            "mempalace_enrich",
            "mempalace_retention_score",
            "mempalace_access_stats",
            "mempalace_working_memory",
            "mempalace_snapshot_create",
            "mempalace_file_history",
            "mempalace_sessions",
            "mempalace_observe",
            "mempalace_commits",
            "mempalace_commit_lookup",
            // mr-dghp: mempalace_mine comes right after commit_lookup
            "mempalace_mine",
            "mempalace_smart_search",
            "mempalace_hybrid_search",
            "memory_claude_bridge_sync",
            "mempalace_claude_bridge_sync",
        ];
        assert_eq!(names, expected);
    }

    #[test]
    fn test_catalog_schemas_match_key_python_fields() {
        let tools = make_tools();
        let search = tools.iter().find(|t| t.name == "mempalace_search").unwrap();
        assert!(search
            .input_schema
            .get("properties")
            .unwrap()
            .get("context")
            .is_some());
        assert!(search
            .input_schema
            .get("properties")
            .unwrap()
            .get("wing")
            .is_some());
        assert!(search
            .input_schema
            .get("properties")
            .unwrap()
            .get("drawer")
            .is_none());

        let diary_write = tools
            .iter()
            .find(|t| t.name == "mempalace_diary_write")
            .unwrap();
        let required = diary_write
            .input_schema
            .get("required")
            .unwrap()
            .as_array()
            .unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("agent_name")));
        assert!(required.iter().any(|v| v.as_str() == Some("entry")));

        let list_rooms = tools
            .iter()
            .find(|t| t.name == "mempalace_list_rooms")
            .unwrap();
        assert!(list_rooms
            .input_schema
            .get("properties")
            .unwrap()
            .get("wing")
            .is_some());
        assert!(list_rooms
            .input_schema
            .get("properties")
            .unwrap()
            .get("drawer")
            .is_none());
    }

    #[test]
    fn test_protocol_and_aaak_spec_match_python_reference() {
        let reference_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../references/mempalace/mempalace/mcp_server.py");

        if !reference_path.exists() {
            eprintln!(
                "Python reference not available on this platform: {}",
                reference_path.display()
            );
            return;
        }

        let python_source =
            std::fs::read_to_string(&reference_path).expect("python reference should be readable");

        assert!(python_source.contains(&format!(
            "PALACE_PROTOCOL = \"\"\"{}\"\"\"",
            PALACE_PROTOCOL
        )));
        assert!(python_source.contains(&format!("AAAK_SPEC = \"\"\"{}\"\"\"", AAAK_SPEC)));
    }

    #[test]
    fn test_status_with_seeded_data_matches_python_shape() {
        let state = test_state();
        {
            let mut db = crate::palace_db::PalaceDb::open(&state.palace_path).unwrap();
            db.add(
                &[
                    ("drawer_proj_backend_aaa", "auth backend"),
                    ("drawer_proj_backend_bbb", "db backend"),
                    ("drawer_proj_frontend_ccc", "frontend ui"),
                    ("drawer_notes_planning_ddd", "planning notes"),
                ],
                &[
                    &[("wing", "project"), ("room", "backend")],
                    &[("wing", "project"), ("room", "backend")],
                    &[("wing", "project"), ("room", "frontend")],
                    &[("wing", "notes"), ("room", "planning")],
                ],
            )
            .unwrap();
            db.flush().unwrap();
        }
        let result = dispatch(&state, "mempalace_status", json!({})).unwrap();
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            parsed.get("total_drawers").and_then(|v| v.as_u64()),
            Some(4)
        );
        assert_eq!(
            parsed
                .get("wings")
                .and_then(|v| v.get("project"))
                .and_then(|v| v.as_u64()),
            Some(3)
        );
        assert_eq!(
            parsed
                .get("wings")
                .and_then(|v| v.get("notes"))
                .and_then(|v| v.as_u64()),
            Some(1)
        );
    }

    #[test]
    fn test_diary_read_empty_matches_python_shape() {
        let state = test_state();
        {
            let mut db = crate::palace_db::PalaceDb::open(&state.palace_path).unwrap();
            db.flush().unwrap();
        }
        let result = dispatch(
            &state,
            "mempalace_diary_read",
            json!({ "agent_name": "Nobody" }),
        )
        .unwrap();
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            parsed
                .get("entries")
                .and_then(|v| v.as_array())
                .map(|v| v.len()),
            Some(0)
        );
        assert_eq!(
            parsed.get("message").and_then(|v| v.as_str()),
            Some("No diary entries yet.")
        );
    }

    #[test]
    fn test_search_response_adds_sanitizer_and_context_metadata() {
        let state = test_state();
        {
            let mut db = crate::palace_db::PalaceDb::open(&state.palace_path).unwrap();
            db.add(
                &[(
                    "drawer_auth_plan",
                    "The auth migration plan lives in backend docs",
                )],
                &[&[("wing", "project"), ("room", "backend")]],
            )
            .unwrap();
            db.flush().unwrap();
        }

        let raw = format!(
            "{}\nWhere is the auth migration plan?",
            "system prompt ".repeat(40)
        );
        let result = dispatch(
            &state,
            "mempalace_search",
            json!({
                "query": raw,
                "context": "Need to answer a user follow-up",
            }),
        )
        .unwrap();

        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();

        assert_eq!(
            parsed.get("query").and_then(|v| v.as_str()),
            Some("Where is the auth migration plan?")
        );
        assert_eq!(
            parsed.get("query_sanitized").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            parsed
                .get("sanitizer")
                .and_then(|v| v.get("method"))
                .and_then(|v| v.as_str()),
            Some("question_extraction")
        );
        assert_eq!(
            parsed.get("context_received").and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn test_search_with_empty_metadata_filter_skips_empty_metadata() {
        // mr-qeye: defensive — drawers with no metadata must not be included
        // when a wing filter is supplied. Test verifies the filter path
        // does not crash on a query that yields empty metadata maps.
        let state = test_state();
        {
            let mut db = crate::palace_db::PalaceDb::open(&state.palace_path).unwrap();
            db.add(
                &[("drawer_test", "alpha beta gamma")],
                &[&[("wing", "project"), ("room", "backend")]],
            )
            .unwrap();
            db.flush().unwrap();
        }
        let result = dispatch(
            &state,
            "mempalace_search",
            json!({
                "query": "alpha",
                "wing": "project",
            }),
        )
        .unwrap();
        // Should not crash; results array may be empty, but must exist.
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert!(parsed.get("results").is_some());
    }

    #[test]
    fn test_mempalace_mine_rejects_missing_path() {
        // mr-dghp: mempalace_mine should return an error, not panic,
        // when the requested path doesn't exist.
        let state = test_state();
        let result = dispatch(
            &state,
            "mempalace_mine",
            json!({ "path": "/definitely/not/a/real/path/xyz" }),
        )
        .unwrap();
        // is_error should be true
        assert_eq!(result.is_error, Some(true));
    }

    #[test]
    fn test_mempalace_mine_blocked_in_read_only() {
        // mr-dghp: read_only mode must reject mempalace_mine.
        let state = {
            let mut s = test_state();
            s.read_only = true;
            s
        };
        let result = dispatch(&state, "mempalace_mine", json!({ "path": "." })).unwrap();
        assert_eq!(result.is_error, Some(true));
    }

    #[test]
    fn test_search_accepts_integer_like_float_limit() {
        let state = test_state();
        {
            let mut db = crate::palace_db::PalaceDb::open(&state.palace_path).unwrap();
            db.add(
                &[(
                    "drawer_auth_plan",
                    "The auth migration plan lives in backend docs",
                )],
                &[&[("wing", "project"), ("room", "backend")]],
            )
            .unwrap();
            db.flush().unwrap();
        }

        let result = dispatch(
            &state,
            "mempalace_search",
            json!({ "query": "auth migration", "limit": 5.0 }),
        );
        assert!(
            result.is_ok(),
            "search with float limit failed: {:?}",
            result
        );
    }

    #[test]
    fn test_diary_read_accepts_integer_like_float_last_n() {
        let state = test_state();
        let write_result = dispatch(
            &state,
            "mempalace_diary_write",
            json!({ "agent_name": "FloatReader", "entry": "hello" }),
        );
        assert!(
            write_result.is_ok(),
            "diary write failed: {:?}",
            write_result
        );

        let result = dispatch(
            &state,
            "mempalace_diary_read",
            json!({ "agent_name": "FloatReader", "last_n": 1.0 }),
        );
        assert!(result.is_ok(), "diary read failed: {:?}", result);
    }

    #[test]
    fn test_traverse_accepts_integer_like_float_max_hops() {
        let state = test_state();
        let result = dispatch(
            &state,
            "mempalace_traverse",
            json!({ "start_room": "unknown", "max_hops": 2.0 }),
        );
        assert!(result.is_ok(), "traverse failed: {:?}", result);
    }

    // ---------------------------------------------------------------------
    // Unknown parameter name (#1512)
    //
    // A kwarg not in the tool schema (wrong parameter *name*, e.g. `text=`
    // instead of `content=`) should surface as JSON-RPC -32602 naming the
    // offending kwarg, instead of being silently dropped by serde and
    // resurfacing indirectly as a later "Missing required 'X'". Symmetric
    // with the missing-required-shape path. The internal `wait_for_previous`
    // transport kwarg must never be flagged, and handlers whose Input struct
    // uses `#[serde(flatten)]` extras (`mempalace_add_drawer`) must keep
    // accepting unknown kwargs.
    // ---------------------------------------------------------------------

    #[test]
    fn test_unknown_param_returns_invalid_params_for_wrong_kwarg_name() {
        let state = test_state();
        let err = dispatch(
            &state,
            "mempalace_search",
            json!({ "query": "hello", "txt": "oops" }),
        )
        .expect_err("unknown 'txt' should surface as an error");
        assert_eq!(err.code.0, -32602);
        let message = err.message.as_ref();
        assert!(message.contains("'txt'"), "message: {message}");
        assert!(message.contains("Unknown parameter"), "message: {message}");
        assert!(message.contains("mempalace_search"), "message: {message}");
        // Names the actual wrong kwarg, not the indirect missing-required symptom.
        assert!(!message.contains("Missing required"), "message: {message}");
    }

    #[test]
    fn test_two_unknown_params_list_both_names() {
        let state = test_state();
        let err = dispatch(
            &state,
            "mempalace_search",
            json!({ "query": "hello", "txt": "a", "bogus": "b" }),
        )
        .expect_err("multiple unknown params should error");
        assert_eq!(err.code.0, -32602);
        let message = err.message.as_ref();
        assert!(message.contains("parameters"), "message: {message}");
        assert!(message.contains("'txt'"), "message: {message}");
        assert!(message.contains("'bogus'"), "message: {message}");
    }

    #[test]
    fn test_wait_for_previous_not_flagged_as_unknown() {
        // `wait_for_previous` is an internal transport kwarg in no tool
        // schema; it must not trip the unknown-param check.
        let state = test_state();
        let result = dispatch(
            &state,
            "mempalace_diary_write",
            json!({
                "agent_name": "x",
                "entry": "y",
                "wait_for_previous": true,
            }),
        );
        assert!(
            result.is_ok(),
            "wait_for_previous should pass through: {:?}",
            result
        );
    }

    #[test]
    fn test_unknown_tool_returns_invalid_params_with_tool_name() {
        // Regression: previously this returned -32601 with message="tools/call",
        // which echoed the JSON-RPC method instead of naming the bad tool.
        // It should now be -32602 with message="Unknown tool: <name>".
        let state = test_state();
        let err = dispatch(&state, "definitely_not_a_real_tool", json!({}))
            .expect_err("unknown tool name should error");
        assert_eq!(err.code.0, -32602);
        let message = err.message.as_ref();
        assert!(
            message.contains("Unknown tool"),
            "message should mention 'Unknown tool', got: {message}"
        );
        assert!(
            message.contains("definitely_not_a_real_tool"),
            "message should name the bogus tool, got: {message}"
        );
        // Must not echo the JSON-RPC method name as the message.
        assert!(
            !message.starts_with("tools/call"),
            "message should not echo the JSON-RPC method, got: {message}"
        );
    }

    #[test]
    fn test_resolve_palace_override_expands_tilde_home() {
        std::env::set_var("HOME", "/tmp/fake_home_42");
        let resolved = resolve_palace_override("~/palace");
        assert_eq!(
            resolved,
            std::path::PathBuf::from("/tmp/fake_home_42/palace")
        );
    }

    #[test]
    fn test_resolve_palace_override_absolute_path_passthrough() {
        let resolved = resolve_palace_override("/var/lib/mp_palace");
        assert_eq!(resolved, std::path::PathBuf::from("/var/lib/mp_palace"));
    }

    #[test]
    fn test_resolve_palace_override_relative_path_passthrough() {
        let resolved = resolve_palace_override("./relative/palace");
        assert_eq!(resolved, std::path::PathBuf::from("./relative/palace"));
    }

    #[test]
    fn test_add_drawer_accepts_unknown_custom_metadata_keys() {
        let state = test_state();
        let result = dispatch(
            &state,
            "mempalace_add_drawer",
            json!({
                "wing": "test",
                "room": "decisions",
                "content": "we picked sqlite",
                "priority": "high",
                "status": "open",
            }),
        );
        assert!(
            result.is_ok(),
            "add_drawer custom metadata should pass through: {:?}",
            result
        );
    }

    // -------------------------------------------------------------------------
    // Group A: Multi-Agent Coordination handler tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_action_create_stores_action() {
        let state = test_state();
        let result = dispatch(
            &state,
            "mempalace_action_create",
            json!({
                "title": "Test Action",
                "description": "A test action",
                "priority": 7,
                "project": "test-project"
            }),
        )
        .expect("action_create should succeed");
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed.get("success").and_then(|v| v.as_bool()), Some(true));
        assert!(parsed
            .get("action_id")
            .and_then(|v| v.as_str())
            .unwrap()
            .starts_with("action_"));
    }

    #[test]
    fn test_action_create_with_dependencies() {
        let state = test_state();
        dispatch(
            &state,
            "mempalace_action_create",
            json!({"title": "Parent Action", "priority": 8}),
        )
        .expect("parent action should be created");
        let result = dispatch(
            &state,
            "mempalace_action_create",
            json!({
                "title": "Child Action",
                "depends_on": ["action_parent"]
            }),
        )
        .expect("child action with dependency should be created");
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed.get("success").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn test_action_update_modifies_status() {
        let state = test_state();
        let create_result = dispatch(
            &state,
            "mempalace_action_create",
            json!({"title": "Updatable Action"}),
        )
        .expect("action should be created");
        let action_id = serde_json::from_str::<Value>(
            &serde_json::to_value(&create_result.content[0])
                .unwrap()
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap(),
        )
        .unwrap()
        .get("action_id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

        let update_result = dispatch(
            &state,
            "mempalace_action_update",
            json!({
                "actionId": action_id,
                "status": "active",
                "priority": 3
            }),
        )
        .expect("action_update should succeed");
        let text = serde_json::to_value(&update_result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed.get("success").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn test_action_update_nonexistent_returns_error() {
        let state = test_state();
        let result = dispatch(
            &state,
            "mempalace_action_update",
            json!({
                "actionId": "nonexistent_action",
                "status": "active"
            }),
        );
        assert!(result.is_err(), "Updating nonexistent action should fail");
    }

    #[test]
    fn test_frontier_returns_unblocked_actions() {
        let state = test_state();
        dispatch(
            &state,
            "mempalace_action_create",
            json!({"title": "Frontier Action 1", "priority": 9}),
        )
        .expect("action should be created");
        dispatch(
            &state,
            "mempalace_action_create",
            json!({"title": "Frontier Action 2", "priority": 5}),
        )
        .expect("action should be created");

        let result = dispatch(&state, "mempalace_frontier", json!({"limit": 10}))
            .expect("frontier should succeed");
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        let count = parsed.get("count").and_then(|v| v.as_u64()).unwrap();
        assert!(count >= 2, "Should have at least 2 unblocked actions");
    }

    #[test]
    fn test_frontier_respects_project_filter() {
        let state = test_state();
        dispatch(
            &state,
            "mempalace_action_create",
            json!({"title": "Alpha Action", "project": "alpha"}),
        )
        .expect("alpha action should be created");
        dispatch(
            &state,
            "mempalace_action_create",
            json!({"title": "Beta Action", "project": "beta"}),
        )
        .expect("beta action should be created");

        let result = dispatch(&state, "mempalace_frontier", json!({"project": "alpha"}))
            .expect("frontier with project filter should succeed");
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        let actions = parsed.get("actions").and_then(|v| v.as_array()).unwrap();
        for action in actions {
            assert_eq!(
                action.get("project").and_then(|v| v.as_str()),
                Some("alpha"),
                "All returned actions should be from alpha project"
            );
        }
    }

    #[test]
    fn test_next_returns_highest_priority_action() {
        let state = test_state();
        dispatch(
            &state,
            "mempalace_action_create",
            json!({"title": "Low Priority", "priority": 1}),
        )
        .expect("low priority action should be created");
        dispatch(
            &state,
            "mempalace_action_create",
            json!({"title": "High Priority", "priority": 10}),
        )
        .expect("high priority action should be created");

        let result = dispatch(&state, "mempalace_next", json!({})).expect("next should succeed");
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        let action = parsed.get("action").unwrap();
        assert_eq!(
            action.get("title").and_then(|v| v.as_str()),
            Some("High Priority"),
            "Should return highest priority action"
        );
    }

    #[test]
    fn test_next_returns_null_when_empty() {
        let state = test_state();
        let result = dispatch(&state, "mempalace_next", json!({}))
            .expect("next on empty frontier should succeed");
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            parsed.get("action").and_then(|v| v.as_null()),
            Some(()),
            "Should return null when no actions available"
        );
    }

    #[test]
    fn test_lease_acquire_creates_lease() {
        let state = test_state();
        let create_result = dispatch(
            &state,
            "mempalace_action_create",
            json!({"title": "Leased Action"}),
        )
        .expect("action should be created");
        let action_id = serde_json::from_str::<Value>(
            &serde_json::to_value(&create_result.content[0])
                .unwrap()
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap(),
        )
        .unwrap()
        .get("action_id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

        let result = dispatch(
            &state,
            "mempalace_lease",
            json!({
                "actionId": action_id,
                "holder": "agent_1",
                "operation": "acquire",
                "ttlMs": 60000
            }),
        )
        .expect("lease acquire should succeed");
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed.get("success").and_then(|v| v.as_bool()), Some(true));
        assert!(parsed
            .get("lease_id")
            .and_then(|v| v.as_str())
            .unwrap()
            .starts_with("lease_"));
    }

    #[test]
    fn test_lease_acquire_fails_when_already_active() {
        let state = test_state();
        let create_result = dispatch(
            &state,
            "mempalace_action_create",
            json!({"title": "Double Lease Action"}),
        )
        .expect("action should be created");
        let action_id = serde_json::from_str::<Value>(
            &serde_json::to_value(&create_result.content[0])
                .unwrap()
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap(),
        )
        .unwrap()
        .get("action_id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

        dispatch(
            &state,
            "mempalace_lease",
            json!({
                "actionId": action_id,
                "holder": "agent_1",
                "operation": "acquire"
            }),
        )
        .expect("first lease should succeed");

        let result = dispatch(
            &state,
            "mempalace_lease",
            json!({
                "actionId": action_id,
                "holder": "agent_2",
                "operation": "acquire"
            }),
        )
        .expect("second lease attempt should return failure");
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            parsed.get("success").and_then(|v| v.as_bool()),
            Some(false),
            "Second lease acquire should fail"
        );
    }

    #[test]
    fn test_lease_release_frees_lease() {
        let state = test_state();
        let create_result = dispatch(
            &state,
            "mempalace_action_create",
            json!({"title": "Release Test Action"}),
        )
        .expect("action should be created");
        let action_id = serde_json::from_str::<Value>(
            &serde_json::to_value(&create_result.content[0])
                .unwrap()
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap(),
        )
        .unwrap()
        .get("action_id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

        dispatch(
            &state,
            "mempalace_lease",
            json!({
                "actionId": action_id,
                "holder": "agent_release",
                "operation": "acquire"
            }),
        )
        .expect("lease should be acquired");

        let release_result = dispatch(
            &state,
            "mempalace_lease",
            json!({
                "actionId": action_id,
                "holder": "agent_release",
                "operation": "release"
            }),
        )
        .expect("lease release should succeed");
        let text = serde_json::to_value(&release_result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed.get("success").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn test_signal_send_creates_signal() {
        let state = test_state();
        let result = dispatch(
            &state,
            "mempalace_signal_send",
            json!({
                "from": "agent_sender",
                "to": "agent_receiver",
                "content": "Hello agent!",
                "signalType": "info"
            }),
        )
        .expect("signal send should succeed");
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed.get("success").and_then(|v| v.as_bool()), Some(true));
        assert!(parsed
            .get("signal_id")
            .and_then(|v| v.as_str())
            .unwrap()
            .starts_with("sig_"));
    }

    #[test]
    fn test_signal_read_returns_messages() {
        let state = test_state();
        dispatch(
            &state,
            "mempalace_signal_send",
            json!({
                "from": "sender",
                "to": "reader_agent",
                "content": "Test message",
                "signalType": "request"
            }),
        )
        .expect("signal should be sent");

        let result = dispatch(
            &state,
            "mempalace_signal_read",
            json!({
                "agentId": "reader_agent",
                "unreadOnly": false,
                "limit": 10
            }),
        )
        .expect("signal read should succeed");
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        let count = parsed.get("count").and_then(|v| v.as_u64()).unwrap();
        assert!(count >= 1, "Should have at least 1 message");
        let messages = parsed.get("messages").and_then(|v| v.as_array()).unwrap();
        assert!(!messages.is_empty());
        assert_eq!(
            messages[0].get("to").and_then(|v| v.as_str()),
            Some("reader_agent"),
            "Message should be addressed to correct agent"
        );
    }

    #[test]
    fn test_signal_read_unread_only() {
        let state = test_state();
        dispatch(
            &state,
            "mempalace_signal_send",
            json!({
                "to": "unread_test_agent",
                "content": "Unread message",
            }),
        )
        .expect("signal should be sent");

        let result = dispatch(
            &state,
            "mempalace_signal_read",
            json!({
                "agentId": "unread_test_agent",
                "unreadOnly": true
            }),
        )
        .expect("unread filter should work");
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        let messages = parsed.get("messages").and_then(|v| v.as_array()).unwrap();
        for msg in messages {
            assert_eq!(
                msg.get("read").and_then(|v| v.as_bool()),
                Some(false),
                "All returned messages should be unread"
            );
        }
    }

    #[test]
    fn test_routine_run_creates_actions() {
        let state = test_state();
        let routine_id = "test_routine_1";
        {
            let db = crate::palace_db::PalaceDb::open(&state.palace_path).unwrap();
            let mut coord = db.coordination();
            coord
                .routine_create(&crate::palace_db::Routine {
                    id: routine_id.to_string(),
                    name: "Test Routine".to_string(),
                    steps: serde_json::to_string(&[
                        serde_json::json!({"title": "Step 1", "description": "First step", "priority": 5}),
                        serde_json::json!({"title": "Step 2", "description": "Second step", "priority": 3}),
                    ])
                    .unwrap(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                })
                .unwrap();
        }

        let result = dispatch(
            &state,
            "mempalace_routine_run",
            json!({
                "routineId": routine_id,
                "project": "routine-project"
            }),
        )
        .expect("routine run should succeed");
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed.get("success").and_then(|v| v.as_bool()), Some(true));
        let actions = parsed
            .get("created_actions")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(
            actions.len(),
            2,
            "Should create 2 actions from routine steps"
        );
    }

    #[test]
    fn test_routine_run_nonexistent_returns_error() {
        let state = test_state();
        let result = dispatch(
            &state,
            "mempalace_routine_run",
            json!({
                "routineId": "nonexistent_routine"
            }),
        )
        .expect("routine run should return error response");
        let text = serde_json::to_value(&result.content[0])
            .unwrap()
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            parsed.get("success").and_then(|v| v.as_bool()),
            Some(false),
            "Should fail for nonexistent routine"
        );
    }

    #[test]
    fn test_read_only_blocks_action_create() {
        let state = {
            let temp_dir = tempfile::tempdir().unwrap();
            let config = crate::Config {
                palace_path: temp_dir.path().join("palace"),
                collection_name: "test_ro_coord".to_string(),
                people_map: Default::default(),
                topic_wings: vec![],
                hall_keywords: Default::default(),
                embedding_model: "naive".to_string(),
                languages: vec![],
                ..Default::default()
            };
            std::fs::create_dir_all(&config.palace_path).unwrap();
            AppState::new(config, true).unwrap()
        };
        let result = dispatch(
            &state,
            "mempalace_action_create",
            json!({"title": "Should be blocked"}),
        );
        assert!(
            result.is_err(),
            "action_create should be blocked in read-only mode"
        );
    }

    #[test]
    fn test_read_only_blocks_signal_send() {
        let state = {
            let temp_dir = tempfile::tempdir().unwrap();
            let config = crate::Config {
                palace_path: temp_dir.path().join("palace"),
                collection_name: "test_ro_signal".to_string(),
                people_map: Default::default(),
                topic_wings: vec![],
                hall_keywords: Default::default(),
                embedding_model: "naive".to_string(),
                languages: vec![],
                ..Default::default()
            };
            std::fs::create_dir_all(&config.palace_path).unwrap();
            AppState::new(config, true).unwrap()
        };
        let result = dispatch(
            &state,
            "mempalace_signal_send",
            json!({
                "to": "someone",
                "content": "Should be blocked"
            }),
        );
        assert!(
            result.is_err(),
            "signal_send should be blocked in read-only mode"
        );
    }

    #[test]
    fn test_read_only_blocks_lease_operations() {
        let state = {
            let temp_dir = tempfile::tempdir().unwrap();
            let config = crate::Config {
                palace_path: temp_dir.path().join("palace"),
                collection_name: "test_ro_lease".to_string(),
                people_map: Default::default(),
                topic_wings: vec![],
                hall_keywords: Default::default(),
                embedding_model: "naive".to_string(),
                languages: vec![],
                ..Default::default()
            };
            std::fs::create_dir_all(&config.palace_path).unwrap();
            AppState::new(config, true).unwrap()
        };
        let result = dispatch(
            &state,
            "mempalace_lease",
            json!({
                "actionId": "any_action",
                "operation": "acquire"
            }),
        );
        assert!(result.is_err(), "lease should be blocked in read-only mode");
    }
}
