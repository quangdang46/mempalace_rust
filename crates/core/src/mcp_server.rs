//! MCP server implementation for MemPalace.
//!
//! Exposes MemPalace functionality as MCP tools via stdio transport.
//! Read-only mode restricts mutations (diary_write, config_write, people_write).

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use rmcp::model::{
    CallToolResult, Content, Implementation, InitializeResult, JsonObject, ListToolsResult,
    ServerCapabilities, ServerInfo as McpServerInfo,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WalEntry {
    timestamp: String,
    tool: String,
    args: serde_json::Value,
    result_summary: Option<serde_json::Value>,
    trace_id: String,
}

fn wal_dir_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("mempalace").join("wal");
        }
    }

    if let Some(proj) = directories::ProjectDirs::from("com", "mempalace", "mempalace") {
        if let Some(state_dir) = proj.state_dir() {
            return state_dir.join("wal");
        }
    }

    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".mempalace")
        .join("wal")
}

fn wal_file_path() -> PathBuf {
    wal_dir_path().join("write_log.jsonl")
}

fn append_wal_entry(entry: &WalEntry) -> anyhow::Result<()> {
    let wal_file = wal_file_path();
    let wal_dir = wal_file
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(wal_dir_path);
    fs::create_dir_all(&wal_dir)?;

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
) {
    let entry = WalEntry {
        timestamp: chrono::Utc::now().to_rfc3339(),
        tool: tool.to_string(),
        args: serde_json::Value::Object(args.clone()),
        result_summary,
        trace_id: trace_id.to_string(),
    };

    if let Err(err) = append_wal_entry(&entry) {
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

pub struct AppState {
    pub config: crate::Config,
    pub db: crate::palace_db::PalaceDb,
    pub read_only: bool,
    pub palace_path: std::path::PathBuf,
}

impl AppState {
    pub fn new(config: crate::Config, read_only: bool) -> anyhow::Result<Self> {
        let palace_path = config.palace_path.clone();
        let db = crate::palace_db::PalaceDb::open(&palace_path)?;
        Ok(Self {
            config,
            db,
            read_only,
            palace_path,
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
    log_tool_invocation(&tool_name, &args, None, &trace_id);
    let result = dispatch(tool_name.clone(), args.clone()).await;
    log_tool_invocation(
        &tool_name,
        &args,
        Some(summarize_tool_result(&result)),
        &trace_id,
    );
    result
}

fn make_dispatch(state: Arc<AppState>) -> impl Fn(String, JsonObject) -> DynResult {
    move |name, args| {
        let state = state.clone();
        Box::pin(async move {
            match name.as_str() {
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
                "mempalace_traverse" => tool_traverse(&state, args),
                "mempalace_find_tunnels" => tool_find_tunnels(&state, args),
                "mempalace_graph_stats" => tool_graph_stats(&state, args),
                "mempalace_diary_read" => tool_diary_read(&state, args),
                "mempalace_diary_write" => tool_diary_write(&state, args),
                _ => Err(ErrorData::method_not_found::<
                    rmcp::model::CallToolRequestMethod,
                >()),
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
            serde_json::json!({ "type": "object", "properties": { "entity": { "type": "string", "description": "Entity to query (e.g. 'Max', 'MyProject', 'Alice')" }, "as_of": { "type": "string", "description": "Date filter — only facts valid at this date (YYYY-MM-DD, optional)" }, "direction": { "type": "string", "description": "outgoing (entity→?), incoming (?→entity), or both (default: both)" } }, "required": ["entity"] }),
        ),
        tool(
            "mempalace_kg_add",
            "KG Add",
            "Add a fact to the knowledge graph. Subject → predicate → object with optional time window. E.g. ('Max', 'started_school', 'Year 7', valid_from='2026-09-01').",
            serde_json::json!({ "type": "object", "properties": { "subject": { "type": "string", "description": "The entity doing/being something" }, "predicate": { "type": "string", "description": "The relationship type (e.g. 'loves', 'works_on', 'daughter_of')" }, "object": { "type": "string", "description": "The entity being connected to" }, "valid_from": { "type": "string", "description": "When this became true (YYYY-MM-DD, optional)" }, "source_closet": { "type": "string", "description": "Closet ID where this fact appears (optional)" } }, "required": ["subject", "predicate", "object"] }),
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
            "mempalace_graph_stats",
            "Graph Stats",
            "Palace graph overview: total rooms, tunnel connections, edges between wings.",
            serde_json::json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        tool(
            "mempalace_search",
            "Search",
            "Semantic search. Returns verbatim drawer content with similarity scores.",
            serde_json::json!({ "type": "object", "properties": { "query": { "type": "string", "description": "What to search for" }, "limit": { "type": "integer", "description": "Max results (default 5)" }, "wing": { "type": "string", "description": "Filter by wing (optional)" }, "room": { "type": "string", "description": "Filter by room (optional)" }, "context": { "type": "string", "description": "Optional caller context for transparency metadata" } }, "required": ["query"] }),
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
            "File verbatim content into the palace. Checks for duplicates first.",
            serde_json::json!({ "type": "object", "properties": { "wing": { "type": "string", "description": "Wing (project name)" }, "room": { "type": "string", "description": "Room (aspect: backend, decisions, meetings...)" }, "content": { "type": "string", "description": "Verbatim content to store — exact words, never summarized" }, "source_file": { "type": "string", "description": "Where this came from (optional)" }, "added_by": { "type": "string", "description": "Who is filing this (default: mcp)" } }, "required": ["wing", "room", "content"] }),
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
            serde_json::json!({ "type": "object", "properties": { "agent_name": { "type": "string", "description": "Your name — each agent gets their own diary wing" }, "entry": { "type": "string", "description": "Your diary entry in AAAK format — compressed, entity-coded, emotion-marked" }, "topic": { "type": "string", "description": "Topic tag (optional, default: general)" } }, "required": ["agent_name", "entry"] }),
        ),
        tool(
            "mempalace_diary_read",
            "Diary Read",
            "Read your recent diary entries (in AAAK). See what past versions of yourself recorded — your journal across sessions.",
            serde_json::json!({ "type": "object", "properties": { "agent_name": { "type": "string", "description": "Your name — each agent gets their own diary wing" }, "last_n": { "type": "integer", "description": "Number of recent entries to read (default: 10)" } }, "required": ["agent_name"] }),
        ),
    ]
}

// ---------------------------------------------------------------------------
// ServerHandler impl
// ---------------------------------------------------------------------------

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
        make_tools().into_iter().find(|t| t.name.as_ref() == name)
    }

    fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        _ctx: rmcp::service::RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, rmcp::ErrorData>> + MaybeSendFuture + '_
    {
        let dispatch = make_dispatch(self.state.clone());
        async move {
            invoke_with_wal(
                request.name.to_string(),
                request.arguments.unwrap_or_default(),
                dispatch,
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
        std::future::ready(Ok(ListToolsResult::with_all_items(make_tools())))
    }
}

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
// Tool handlers
// ---------------------------------------------------------------------------

fn tool_status(state: &AppState, _args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    let entries = state.db.get_all(None, None, 10_000);
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
        "total_drawers": state.db.count(),
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
    let entries = state.db.get_all(None, None, 10_000);
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
    let entries = state.db.get_all(input.wing.as_deref(), None, 10_000);
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
    let entries = state.db.get_all(None, None, 10_000);
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
    }
    let input: Input = parse_args_with_integer_coercion(args, &["limit"])?;
    let sanitized = crate::query_sanitizer::sanitize_query(&input.query);
    let db = crate::palace_db::PalaceDb::open(&state.palace_path)
        .map_err(|e| internal_error_safe(&e))?;
    let query_results = db
        .query_sync(
            &sanitized.clean_query,
            input.wing.as_deref(),
            input.room.as_deref(),
            input.limit.unwrap_or(5),
        )
        .map_err(|e| internal_error_safe(&e))?;
    let mut response = serde_json::to_value(crate::searcher::SearchResponse {
        query: sanitized.clean_query.clone(),
        filters: crate::searcher::SearchFilters {
            wing: input.wing.clone(),
            room: input.room.clone(),
        },
        results: query_results
            .into_iter()
            .map(crate::searcher::SearchResult::from)
            .collect(),
    })
    .map_err(|e| internal_error_safe(&e))?;

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
    }
    let input: Input = parse_args(args)?;
    let hash = short_hash(
        &format!(
            "{}{}{}",
            input.wing,
            input.room,
            &input.content.chars().take(100).collect::<String>()
        ),
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
    db.add(
        &[(&drawer_id, &input.content)],
        &[&[
            ("wing", &input.wing),
            ("room", &input.room),
            ("source_file", input.source_file.as_deref().unwrap_or("")),
            ("added_by", input.added_by.as_deref().unwrap_or("mcp")),
            ("chunk_index", "0"),
        ]],
    )
    .map_err(|e| internal_error_safe(&e))?;
    db.flush().map_err(|e| internal_error_safe(&e))?;
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
        direction: Option<String>,
    }
    let input: Input = parse_args(args)?;
    let kg = crate::knowledge_graph::KnowledgeGraph::open(&kg_path(state))
        .map_err(|e| internal_error_safe(&e))?;
    let facts = kg
        .query_entity(
            &input.entity,
            input.as_of.as_deref(),
            input.direction.as_deref().unwrap_or("both"),
        )
        .map_err(|e| internal_error_safe(&e))?;
    ok_json(
        serde_json::json!({ "entity": input.entity, "as_of": input.as_of, "facts": facts, "count": facts.len() }),
    )
}

fn tool_kg_add(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    struct Input {
        subject: String,
        predicate: String,
        object: String,
        valid_from: Option<String>,
        source_closet: Option<String>,
    }
    let input: Input = parse_args(args)?;
    let mut kg = crate::knowledge_graph::KnowledgeGraph::open(&kg_path(state))
        .map_err(|e| internal_error_safe(&e))?;
    let triple_id = kg
        .add_triple(
            &input.subject,
            &input.predicate,
            &input.object,
            input.valid_from.as_deref(),
            None,
            None,
            input.source_closet.as_deref(),
            None,
        )
        .map_err(|e| internal_error_safe(&e))?;
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
    let mut kg = crate::knowledge_graph::KnowledgeGraph::open(&kg_path(state))
        .map_err(|e| internal_error_safe(&e))?;
    kg.invalidate(
        &input.subject,
        &input.predicate,
        &input.object,
        input.ended.as_deref(),
    )
    .map_err(|e| internal_error_safe(&e))?;
    ok_json(
        serde_json::json!({ "success": true, "fact": format!("{} → {} → {}", input.subject, input.predicate, input.object), "ended": input.ended.unwrap_or_else(|| "today".to_string()) }),
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

fn build_graph_from_db(state: &AppState) -> crate::palace_graph::PalaceGraph {
    use crate::palace_graph::{HallType, PalaceGraph, Room, Wing, WingType};
    let mut by_wing: HashMap<String, Vec<Room>> = HashMap::new();
    for entry in state.db.get_all(None, None, 10_000) {
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
    let graph = build_graph_from_db(state);
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
    let graph = build_graph_from_db(state);
    let tunnels = graph.find_tunnels(input.wing_a.as_deref(), input.wing_b.as_deref());
    ok_json(tunnels)
}

fn tool_graph_stats(state: &AppState, _args: JsonObject) -> Result<CallToolResult, ErrorData> {
    if collection_missing(state) {
        return ok_json(no_palace());
    }
    let graph = build_graph_from_db(state);
    let stats = graph.stats();
    ok_json(serde_json::json!({
        "total_rooms": stats.total_rooms,
        "tunnel_rooms": stats.tunnel_rooms,
        "total_edges": stats.total_edges,
        "rooms_per_wing": stats.rooms_per_wing,
        "top_tunnels": stats.top_tunnels,
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
    }
    let input: Input = parse_args_with_integer_coercion(args, &["last_n"])?;
    let wing = format!("wing_{}", input.agent_name.to_lowercase().replace(' ', "_"));
    let entries = state.db.get_all(Some(&wing), Some("diary"), 10_000);
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
            "agent": input.agent_name,
            "entries": [],
            "message": "No diary entries yet.",
        }));
    }
    ok_json(serde_json::json!({
        "agent": input.agent_name,
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
    }
    let input: Input = parse_args(args)?;
    let now = chrono::Local::now();
    let wing = format!("wing_{}", input.agent_name.to_lowercase().replace(' ', "_"));
    let topic = input.topic.unwrap_or_else(|| "general".to_string());
    let id = format!(
        "diary_{}_{}_{}",
        wing,
        now.format("%Y%m%d_%H%M%S"),
        &short_hash(&input.entry.chars().take(50).collect::<String>(), 12)
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
            ("agent", &input.agent_name),
            ("filed_at", &filed_at),
            ("date", &date),
        ]],
    )
    .map_err(|e| internal_error_safe(&e))?;
    db.flush().map_err(|e| internal_error_safe(&e))?;
    ok_json(serde_json::json!({
        "success": true,
        "entry_id": id,
        "agent": input.agent_name,
        "topic": topic,
        "timestamp": filed_at,
    }))
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

pub fn run_server(read_only: bool) -> anyhow::Result<()> {
    let config = crate::Config::load()?;
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn read_wal_entries() -> Vec<WalEntry> {
        let wal_file = wal_file_path();
        let content = std::fs::read_to_string(wal_file).expect("wal file should exist");
        content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str::<WalEntry>(line).expect("wal line should parse"))
            .collect()
    }

    fn test_state() -> AppState {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = crate::Config {
            palace_path: temp_dir.path().join("palace"),
            collection_name: "test_collection".to_string(),
            people_map: Default::default(),
            topic_wings: vec!["emotions".to_string()],
            hall_keywords: Default::default(),
            embedding_model: "naive".to_string(),
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
        };
        let f = make_dispatch(Arc::new(owned_state));
        let args = args.as_object().cloned().unwrap_or_default();
        // Use try_current to detect if we're in a runtime
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => handle.block_on(invoke_with_wal(name.to_string(), args, f)),
            Err(_) => {
                // No runtime: create one just for this call
                let rt = Runtime::new().unwrap();
                rt.block_on(invoke_with_wal(name.to_string(), args, f))
            }
        }
    }

    #[test]
    fn test_status() {
        let state = test_state();
        let result = dispatch(&state, "mempalace_status", json!({}));
        assert!(result.is_ok());
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
    fn test_wal_file_path_prefers_xdg_state_home() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        std::env::set_var("XDG_STATE_HOME", temp.path());
        let path = wal_file_path();
        assert_eq!(
            path,
            temp.path()
                .join("mempalace")
                .join("wal")
                .join("write_log.jsonl")
        );
        std::env::remove_var("XDG_STATE_HOME");
    }

    #[test]
    fn test_dispatch_writes_wal_entries() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let xdg_path = temp.path().join("mempalace_state");
        std::env::set_var("XDG_STATE_HOME", &xdg_path);
        let state = test_state();

        let wal_path = wal_file_path();
        if wal_path.exists() {
            std::fs::remove_file(&wal_path).ok();
        }

        let result = dispatch(&state, "mempalace_status", json!({}));
        assert!(result.is_ok());

        let entries = read_wal_entries();
        assert!(
            entries.len() >= 2,
            "expected at least 2 entries, got {}",
            entries.len()
        );
        assert_eq!(entries[0].tool, "mempalace_status");
        assert_eq!(entries[1].tool, "mempalace_status");
        assert_eq!(entries[0].trace_id, entries[1].trace_id);
        assert!(entries[0].result_summary.is_none());
        assert_eq!(
            entries[1]
                .result_summary
                .as_ref()
                .and_then(|v| v.get("status"))
                .and_then(|v| v.as_str()),
            Some("ok")
        );

        std::env::remove_var("XDG_STATE_HOME");
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
    fn test_catalog_matches_python_surface() {
        let tools = make_tools();
        let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
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
            "mempalace_traverse",
            "mempalace_find_tunnels",
            "mempalace_graph_stats",
            "mempalace_search",
            "mempalace_check_duplicate",
            "mempalace_add_drawer",
            "mempalace_delete_drawer",
            "mempalace_diary_write",
            "mempalace_diary_read",
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
}
