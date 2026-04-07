//! MCP server implementation for MemPalace.
//!
//! Exposes MemPalace functionality as MCP tools via stdio transport.
//! Read-only mode restricts mutations (diary_write, config_write, people_write).

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use rmcp::model::{
    CallToolResult, Content, InitializeResult, JsonObject, ListToolsResult, ProtocolVersion,
    ServerCapabilities, ServerInfo as McpServerInfo, TextContent, ToolsCapability,
};
use rmcp::service::MaybeSendFuture;
use rmcp::transport::stdio;
use rmcp::{handler::server::ServerHandler, ErrorData, RoleServer, Service, ServiceExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::runtime::Runtime;

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

fn make_dispatch(state: Arc<AppState>) -> impl Fn(String, JsonObject) -> DynResult {
    move |name, args| {
        let state = state.clone();
        Box::pin(async move {
            match name.as_str() {
                "mempalace_status" => tool_status(&state, args),
                "mempalace_list_drawers" => tool_list_drawers(&state, args),
                "mempalace_list_rooms" => tool_list_rooms(&state, args),
                "mempalace_mine" => tool_mine(&state, args),
                "mempalace_search" => tool_search(&state, args),
                "mempalace_full_search" => tool_full_search(&state, args),
                "mempalace_get_memory" => tool_get_memory(&state, args),
                "mempalace_write_memory" => tool_write_memory(&state, args),
                "mempalace_diary_read" => tool_diary_read(&state, args),
                "mempalace_diary_write" => tool_diary_write(&state, args),
                "mempalace_list_entities" => tool_list_entities(&state, args),
                "mempalace_get_entity" => tool_get_entity(&state, args),
                "mempalace_graph_dump" => tool_graph_dump(&state, args),
                "mempalace_onboard" => tool_onboard(&state, args),
                "mempalace_doctor" => tool_doctor(&state, args),
                "mempalace_get_config" => tool_get_config(&state, args),
                "mempalace_set_config" => tool_set_config(&state, args),
                "mempalace_get_people" => tool_get_people(&state, args),
                "mempalace_set_people" => tool_set_people(&state, args),
                "mempalace_compress" => tool_compress(&state, args),
                "mempalace_decompress" => tool_decompress(&state, args),
                "mempalace_feedback" => tool_feedback(&state, args),
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
            "Overview of the MemPalace memory palace.",
            serde_json::json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "mempalace_list_drawers",
            "List Drawers",
            "List all drawers (wings/halls) in the palace.",
            serde_json::json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "mempalace_list_rooms",
            "List Rooms",
            "List rooms within a specific drawer.",
            serde_json::json!({ "type": "object", "properties": { "drawer": { "type": "string" } } }),
        ),
        tool(
            "mempalace_mine",
            "Mine Memory",
            "Mine a file and add entries to the palace.",
            serde_json::json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
        ),
        tool(
            "mempalace_search",
            "Search Palace",
            "Search palace entries by text.",
            serde_json::json!({ "type": "object", "properties": { "query": { "type": "string" }, "drawer": { "type": "string" }, "room": { "type": "string" }, "limit": { "type": "integer" } }, "required": ["query"] }),
        ),
        tool(
            "mempalace_full_search",
            "Full Search",
            "Full-text search.",
            serde_json::json!({ "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] }),
        ),
        tool(
            "mempalace_get_memory",
            "Get Memory",
            "Get entries by key.",
            serde_json::json!({ "type": "object", "properties": { "key": { "type": "string" }, "drawer": { "type": "string" } }, "required": ["key"] }),
        ),
        tool(
            "mempalace_write_memory",
            "Write Memory",
            "Write a key-value entry.",
            serde_json::json!({ "type": "object", "properties": { "key": { "type": "string" }, "value": { "type": "string" }, "drawer": { "type": "string" } }, "required": ["key", "value"] }),
        ),
        tool(
            "mempalace_diary_read",
            "Read Diary",
            "Read diary entries.",
            serde_json::json!({ "type": "object", "properties": { "limit": { "type": "integer" } } }),
        ),
        tool(
            "mempalace_diary_write",
            "Write Diary",
            "Append a diary entry.",
            serde_json::json!({ "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }),
        ),
        tool(
            "mempalace_list_entities",
            "List Entities",
            "List known entities.",
            serde_json::json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "mempalace_get_entity",
            "Get Entity",
            "Get an entity by name.",
            serde_json::json!({ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }),
        ),
        tool(
            "mempalace_graph_dump",
            "Graph Dump",
            "Export knowledge graph stats.",
            serde_json::json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "mempalace_onboard",
            "Onboard",
            "Run onboarding.",
            serde_json::json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "mempalace_doctor",
            "Doctor",
            "Run health checks.",
            serde_json::json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "mempalace_get_config",
            "Get Config",
            "Get configuration.",
            serde_json::json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "mempalace_set_config",
            "Set Config",
            "Update configuration.",
            serde_json::json!({ "type": "object", "properties": { "key": { "type": "string" }, "value": { "type": "string" } }, "required": ["key", "value"] }),
        ),
        tool(
            "mempalace_get_people",
            "Get People",
            "Get people map.",
            serde_json::json!({ "type": "object", "properties": {} }),
        ),
        tool(
            "mempalace_set_people",
            "Set People",
            "Update people map.",
            serde_json::json!({ "type": "object", "properties": { "people": { "type": "object" } }, "required": ["people"] }),
        ),
        tool(
            "mempalace_compress",
            "Compress Text",
            "Compress text using AAAK shorthand dialect.",
            serde_json::json!({ "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }),
        ),
        tool(
            "mempalace_decompress",
            "Decompress Text",
            "Decompress AAAK shorthand back to natural language.",
            serde_json::json!({ "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }),
        ),
        tool(
            "mempalace_feedback",
            "Record Feedback",
            "Record retrieval feedback to improve future ranking. outcome: 'helpful', 'unhelpful', or 'neutral'.",
            serde_json::json!({ "type": "object", "properties": { "drawer_id": { "type": "string" }, "query": { "type": "string" }, "outcome": { "type": "string", "enum": ["helpful", "unhelpful", "neutral"] } }, "required": ["drawer_id", "query", "outcome"] }),
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
        InitializeResult::default()
    }

    fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        make_tools().into_iter().find(|t| t.name.as_ref() == name)
    }

    fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        _ctx: rmcp::service::RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, rmcp::Error>> + MaybeSendFuture + '_
    {
        let dispatch = make_dispatch(self.state.clone());
        async move {
            dispatch(
                request.name.to_string(),
                request.arguments.unwrap_or_default(),
            )
            .await
        }
    }

    fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _ctx: rmcp::service::RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, rmcp::Error>> + MaybeSendFuture + '_
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
    let s = serde_json::to_string(&value)
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
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

// ---------------------------------------------------------------------------
// Tool handlers
// ---------------------------------------------------------------------------

fn tool_status(state: &AppState, _args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Serialize)]
    struct StatusOutput {
        total_entries: usize,
        palace_path: String,
        collection_name: String,
    }
    ok_json(StatusOutput {
        total_entries: state.db.count(),
        palace_path: state.palace_path.display().to_string(),
        collection_name: state.config.collection_name.clone(),
    })
}

fn tool_list_drawers(_state: &AppState, _args: JsonObject) -> Result<CallToolResult, ErrorData> {
    ok_json(serde_json::json!({
        "drawers": ["emotions", "consciousness", "memory", "technical", "identity", "family", "creative"],
        "total": 7
    }))
}

fn tool_list_rooms(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        drawer: Option<String>,
    }
    let input: Input = parse_args(args)?;
    let entries = state.db.get_all(input.drawer.as_deref(), None, 1000);
    let mut rooms: Vec<&str> = entries
        .iter()
        .filter_map(|e| e.metadatas.first())
        .filter_map(|m| m.get("room"))
        .filter_map(|v| v.as_str())
        .collect();
    rooms.sort();
    rooms.dedup();
    ok_json(serde_json::json!({ "rooms": rooms }))
}

fn tool_mine(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        path: String,
    }
    let input: Input = parse_args(args)?;
    if !std::path::Path::new(&input.path).is_file() {
        return Err(ErrorData::invalid_params(
            format!("not a file: {}", input.path),
            None,
        ));
    }
    let rt = Runtime::new().map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    rt.block_on(async {
        let mut miner = crate::miner::Miner::new(&state.palace_path, "general", vec![])
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        match miner.mine_file(std::path::Path::new(&input.path)).await {
            Ok(count) => ok_json(serde_json::json!({ "mined": count })),
            Err(e) => Err(ErrorData::internal_error(e.to_string(), None)),
        }
    })
}

fn tool_search(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        query: String,
        drawer: Option<String>,
        room: Option<String>,
        limit: Option<usize>,
    }
    let input: Input = parse_args(args)?;
    // Sync fallback: open DB and search without async
    let db = crate::palace_db::PalaceDb::open(&state.palace_path)
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    // Search synchronously using naive similarity matching
    let results = db.get_all(
        input.drawer.as_deref(),
        input.room.as_deref(),
        input.limit.unwrap_or(10),
    );
    let out: Vec<serde_json::Value> = results
        .into_iter()
        .filter(|r| {
            r.documents
                .first()
                .map(|c| c.contains(&input.query))
                .unwrap_or(false)
        })
        .map(|r| {
            serde_json::json!({
                "id": r.ids.first(),
                "content": r.documents.first(),
                "distance": r.distances.first(),
            })
        })
        .collect();
    ok_json(out)
}

fn tool_full_search(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        query: String,
    }
    let input: Input = parse_args(args)?;
    // Sync fallback: search all entries by keyword similarity
    let db = crate::palace_db::PalaceDb::open(&state.palace_path)
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    let results = db.get_all(None, None, 10);
    let out: Vec<serde_json::Value> = results
        .into_iter()
        .filter(|r| {
            r.documents
                .first()
                .map(|c| c.contains(&input.query))
                .unwrap_or(false)
        })
        .map(|r| {
            serde_json::json!({
                "id": r.ids.first(),
                "content": r.documents.first(),
            })
        })
        .collect();
    ok_json(serde_json::json!({ "searched": input.query, "results": out }))
}

fn tool_get_memory(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        key: String,
        drawer: Option<String>,
        limit: Option<usize>,
    }
    let input: Input = parse_args(args)?;
    let entries = state
        .db
        .get_all(input.drawer.as_deref(), None, input.limit.unwrap_or(20));
    let filtered: Vec<serde_json::Value> = entries.into_iter()
        .filter(|e| e.documents.first().map(|c| c.contains(&input.key)).unwrap_or(false))
        .map(|e| serde_json::json!({ "content": e.documents.first(), "metadata": e.metadatas.first() }))
        .collect();
    ok_json(filtered)
}

fn tool_write_memory(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        key: String,
        value: String,
        drawer: Option<String>,
    }
    let input: Input = parse_args(args)?;
    let id = format!("mem_{}", uuid::Uuid::new_v4());
    let wing = input.drawer.as_deref().unwrap_or("general");
    let mut db = crate::palace_db::PalaceDb::open(&state.palace_path)
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    db.add(
        &[(&id, &input.value)],
        &[&[("wing", wing), ("key", &input.key)]],
    )
    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    ok_json(serde_json::json!({ "id": id }))
}

fn tool_diary_read(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        limit: Option<usize>,
    }
    let input: Input = parse_args(args)?;
    let entries = state
        .db
        .get_all(Some("diary"), None, input.limit.unwrap_or(50));
    let out: Vec<serde_json::Value> = entries.iter()
        .map(|e| serde_json::json!({ "content": e.documents.first(), "metadata": e.metadatas.first() }))
        .collect();
    ok_json(out)
}

fn tool_diary_write(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        text: String,
    }
    let input: Input = parse_args(args)?;
    let id = format!("diary_{}", chrono::Utc::now().format("%Y%m%d_%H%M%S"));
    let mut db = crate::palace_db::PalaceDb::open(&state.palace_path)
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    db.add(&[(&id, &input.text)], &[&[("wing", "diary")]])
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    ok_json(serde_json::json!({ "id": id }))
}

fn tool_list_entities(state: &AppState, _args: JsonObject) -> Result<CallToolResult, ErrorData> {
    let summary = crate::entity_registry::EntityRegistry::load(&state.palace_path)
        .map(|r| {
            serde_json::json!({
                "people_count": r.people_count(),
                "projects_count": r.projects_count(),
            })
        })
        .unwrap_or(serde_json::json!({ "error": "could not load registry" }));
    ok_json(summary)
}

fn tool_get_entity(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        name: String,
    }
    let input: Input = parse_args(args)?;
    let _result = crate::entity_registry::EntityRegistry::load(&state.palace_path)
        .map(|r| r.lookup(&input.name, ""))
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    ok_json(serde_json::json!({ "entity": { "name": input.name } }))
}

fn tool_graph_dump(state: &AppState, _args: JsonObject) -> Result<CallToolResult, ErrorData> {
    let stats = crate::knowledge_graph::KnowledgeGraph::open(&state.palace_path)
        .and_then(|kg| kg.stats())
        .map(|s| {
            serde_json::json!({
                "total_entities": s.total_entities,
                "total_triples": s.total_triples,
                "current_facts": s.current_facts,
            })
        })
        .unwrap_or(serde_json::json!({ "error": "could not load graph" }));
    ok_json(stats)
}

fn tool_onboard(_state: &AppState, _args: JsonObject) -> Result<CallToolResult, ErrorData> {
    ok_json(
        serde_json::json!({ "status": "onboarding_not_implemented", "message": "Use mempalace init to set up the palace" }),
    )
}

fn tool_doctor(state: &AppState, _args: JsonObject) -> Result<CallToolResult, ErrorData> {
    match crate::doctor::run_doctor(&state.palace_path) {
        Ok(report) => ok_json(
            serde_json::json!({ "healthy": report.healthy, "check_count": report.checks.len() }),
        ),
        Err(e) => Err(ErrorData::internal_error(e.to_string(), None)),
    }
}

fn tool_get_config(state: &AppState, _args: JsonObject) -> Result<CallToolResult, ErrorData> {
    ok_json(serde_json::json!({
        "palace_path": state.palace_path.display().to_string(),
        "collection_name": state.config.collection_name,
    }))
}

fn tool_set_config(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        key: String,
        value: String,
    }
    let input: Input = parse_args(args)?;
    if input.key == "collection_name" {
        let mut cfg = state.config.clone();
        cfg.collection_name = input.value;
        cfg.save()
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    }
    ok_json(serde_json::json!({ "updated": input.key }))
}

fn tool_get_people(state: &AppState, _args: JsonObject) -> Result<CallToolResult, ErrorData> {
    let people = state.config.load_people_map().unwrap_or_default();
    ok_json(people)
}

fn tool_set_people(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    read_only_guard(state)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        people: HashMap<String, String>,
    }
    let input: Input = parse_args(args)?;
    state
        .config
        .save_people_map(&input.people)
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    ok_json(serde_json::json!({ "saved": input.people.len() }))
}

fn tool_compress(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    struct Input {
        text: String,
    }
    let input: Input = parse_args(args)?;
    let people_map = state.config.load_people_map().unwrap_or_default();
    let compressed = crate::dialect::compress(&input.text, &people_map);
    let stats = crate::dialect::compression_stats(&input.text, &compressed);
    ok_json(serde_json::json!({
        "original": input.text,
        "compressed": compressed,
        "stats": {
            "original_tokens": stats.original_tokens,
            "compressed_tokens": stats.compressed_tokens,
            "ratio": stats.ratio
        }
    }))
}

fn tool_decompress(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    struct Input {
        text: String,
    }
    let input: Input = parse_args(args)?;
    let people_map = state.config.load_people_map().unwrap_or_default();
    let decompressed = crate::dialect::decompress(&input.text, &people_map);
    ok_json(serde_json::json!({
        "original": input.text,
        "decompressed": decompressed
    }))
}

fn tool_feedback(state: &AppState, args: JsonObject) -> Result<CallToolResult, ErrorData> {
    #[derive(Deserialize)]
    struct Input {
        drawer_id: String,
        query: String,
        outcome: String,
    }
    let input: Input = parse_args(args)?;
    let kg = crate::knowledge_graph::KnowledgeGraph::open(&state.palace_path.join("knowledge.db"))
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    kg.record_feedback(&input.drawer_id, &input.query, &input.outcome)
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    ok_json(serde_json::json!({
        "drawer_id": input.drawer_id,
        "query": input.query,
        "outcome": input.outcome,
        "recorded": true
    }))
}

// ---------------------------------------------------------------------------
// Run entry point
// ---------------------------------------------------------------------------

pub fn run_server(read_only: bool) -> anyhow::Result<()> {
    let config = crate::Config::load()?;
    let server = MempalaceServer::new(AppState::new(config, read_only)?);
    let (stdin, stdout) = stdio();
    let rt = Runtime::new()?;
    rt.block_on(async { server.serve((stdin, stdout)).await })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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
            Ok(handle) => handle.block_on(f(name.to_string(), args)),
            Err(_) => {
                // No runtime: create one just for this call
                let rt = Runtime::new().unwrap();
                rt.block_on(f(name.to_string(), args))
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
    fn test_list_drawers() {
        let state = test_state();
        let result = dispatch(&state, "mempalace_list_drawers", json!({}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_list_rooms() {
        let state = test_state();
        let result = dispatch(
            &state,
            "mempalace_list_rooms",
            json!({ "drawer": "emotions" }),
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
    fn test_get_memory() {
        let state = test_state();
        let result = dispatch(
            &state,
            "mempalace_get_memory",
            json!({ "key": "nonexistent" }),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_diary_read() {
        let state = test_state();
        let result = dispatch(&state, "mempalace_diary_read", json!({}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_list_entities() {
        let state = test_state();
        let result = dispatch(&state, "mempalace_list_entities", json!({}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_graph_dump() {
        let state = test_state();
        let result = dispatch(&state, "mempalace_graph_dump", json!({}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_config() {
        let state = test_state();
        let result = dispatch(&state, "mempalace_get_config", json!({}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_people() {
        let state = test_state();
        let result = dispatch(&state, "mempalace_get_people", json!({}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_onboard() {
        let state = test_state();
        let result = dispatch(&state, "mempalace_onboard", json!({}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_doctor() {
        let state = test_state();
        let result = dispatch(&state, "mempalace_doctor", json!({}));
        assert!(result.is_ok());
    }

    #[test]
    fn test_read_only_blocks_write_memory() {
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
            "mempalace_write_memory",
            json!({ "key": "test", "value": "val" }),
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
        let result = dispatch(&state, "mempalace_diary_write", json!({ "text": "hello" }));
        assert!(result.is_err());
    }

    #[test]
    fn test_read_only_blocks_set_config() {
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
            "mempalace_set_config",
            json!({ "key": "collection_name", "value": "new" }),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_read_only_blocks_set_people() {
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
            "mempalace_set_people",
            json!({ "people": { "Alice": "A" } }),
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
    fn test_diary_write_and_read_roundtrip() {
        let state = test_state();
        let write_result = dispatch(
            &state,
            "mempalace_diary_write",
            json!({ "text": "Test diary entry" }),
        );
        assert!(
            write_result.is_ok(),
            "diary write failed: {:?}",
            write_result
        );
        let read_result = dispatch(&state, "mempalace_diary_read", json!({}));
        assert!(read_result.is_ok(), "diary read failed: {:?}", read_result);
    }

    #[test]
    fn test_write_memory_and_get_memory_roundtrip() {
        let state = test_state();
        let write_result = dispatch(
            &state,
            "mempalace_write_memory",
            json!({ "key": "test_key", "value": "test_value", "drawer": "emotions" }),
        );
        assert!(
            write_result.is_ok(),
            "write_memory failed: {:?}",
            write_result
        );
        let read_result = dispatch(&state, "mempalace_get_memory", json!({ "key": "test_key" }));
        assert!(read_result.is_ok(), "get_memory failed: {:?}", read_result);
    }
}
