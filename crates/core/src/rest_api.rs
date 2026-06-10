//! REST API layer for MemPalace MCP server.
//!
//! Exposes MemPalace functionality as HTTP endpoints wrapping MCP tool functions.
//! Enable via `--http` CLI flag, port configurable via `MEMPALACE_HTTP_PORT` env var.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

use crate::export::disk_size_manager::DiskSizeManager;
use crate::mcp_server::AppState;
use crate::palace_db::MemorySlot;
use rmcp::model::{CallToolResult, Content, JsonObject, RawContent};
use serde_json::json;

// ---------------------------------------------------------------------------
// Shared server state
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct HttpServerState {
    pub app_state: Arc<AppState>,
    pub read_only: bool,
    pub disk_size_manager: Arc<Mutex<DiskSizeManager>>,
    #[cfg(feature = "health")]
    pub embedder: std::sync::Arc<dyn crate::embed::Embedder>,
}

type SharedState = Arc<Mutex<HttpServerState>>;

// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let body = serde_json::json!({
            "error": self.message,
            "status": self.status.as_u16(),
        });
        (self.status, Json(body)).into_response()
    }
}

impl From<rmcp::ErrorData> for ApiError {
    fn from(e: rmcp::ErrorData) -> Self {
        ApiError {
            status: StatusCode::BAD_REQUEST,
            message: e.message.to_string(),
        }
    }
}

impl From<String> for ApiError {
    fn from(s: String) -> Self {
        ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: s,
        }
    }
}

impl From<&str> for ApiError {
    fn from(s: &str) -> Self {
        ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: s.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Request helpers
// ---------------------------------------------------------------------------

fn map_to_json_object(map: HashMap<String, serde_json::Value>) -> JsonObject {
    map.into_iter().collect()
}

fn text_content_to_json(result: CallToolResult) -> serde_json::Value {
    use rmcp::model::{Content, RawContent};
    result.content.first().map(|c| {
        // Content = Annotated<RawContent>, which Derefs to RawContent
        let raw: &RawContent = &c.raw;
        match raw {
            RawContent::Text(ref raw) => serde_json::from_str::<serde_json::Value>(&raw.text).unwrap_or_else(|_| json!({ "text": &raw.text })),
            _ => json!({ "ok": true }),
        }
    }).unwrap_or_else(|| json!({ "ok": true }))
}

// ---------------------------------------------------------------------------
// Tool invocation - call make_dispatch from mcp_server
// ---------------------------------------------------------------------------

async fn invoke_tool(
    state: &HttpServerState,
    tool_name: &str,
    args: JsonObject,
) -> Result<CallToolResult, ApiError> {
    let state = state.app_state.clone();
    let dispatch = crate::mcp_server::make_dispatch(state);
    let result = dispatch(tool_name.to_string(), args).await;
    match result {
        Ok(r) => Ok(r),
        Err(e) => Err(ApiError::from(e)),
    }
}

fn tool_result_to_response(result: Result<CallToolResult, ApiError>) -> axum::response::Response {
    use rmcp::model::Content;
    match result {
        Ok(result) => {
            let content = result.content;
            if content.is_empty() {
                (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
            } else {
                let first = content.first();
                match first {
                    Some(content) if matches!(content.raw, RawContent::Text(_)) => {
                        // Content = Annotated<RawContent>, derefs to RawContent
                        let raw: &RawContent = &content.raw;
                        match raw {
                            RawContent::Text(ref raw) => {
                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw.text) {
                                    (StatusCode::OK, Json(v)).into_response()
                                } else {
                                    (StatusCode::OK, Json(json!({ "result": &raw.text }))).into_response()
                                }
                            }
                            _ => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
                        }
                    }
                    _ => (StatusCode::OK, Json(json!({ "ok": true }))).into_response(),
                }
            }
        }
        Err(e) => (
            e.status,
            Json(json!({
                "error": e.message,
            })),
        ).into_response(),
    }
}

// ---------------------------------------------------------------------------
// Health & Info endpoints
// ---------------------------------------------------------------------------

async fn health_handler() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "service": "mempalace-rest-api",
    }))
}

/// Full health report — all 6 checks aggregated.
/// Requires the `health` feature. Returns 503 if any check is Critical.
#[cfg(feature = "health")]
async fn healthz_handler(State(state): State<SharedState>) -> impl IntoResponse {
    let monitor = crate::health::get_health_monitor();
    let report = monitor.run_all().await;
    let status_code = monitor.to_http_status(&report);
    (StatusCode::from_u16(status_code).expect("valid HTTP status code"), Json(report))
}

/// Fallback healthz when `health` feature is off.
#[cfg(not(feature = "health"))]
async fn healthz_handler() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({"status": "healthy", "note": "health feature not enabled"})))
}

/// Lightweight liveness probe — confirms the process is alive.
/// No `health` feature required.
async fn livez_handler() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({"status": "alive"})))
}

async fn list_tools_handler(
    State(_state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Return available tools
    Ok(Json(json!({
        "tools": [
            "mempalace_recall", "mempalace_save", "mempalace_recall",
            "mempalace_status", "mempalace_observe", "mempalace_enrich",
            "mempalace_consolidate", "mempalace_kg_query", "mempalace_kg_add",
            "mempalace_diary_read", "mempalace_diary_write",
            "mempalace_slot_list", "mempalace_slot_get", "mempalace_slot_create",
            "mempalace_sentinel_list", "mempalace_checkpoint_list",
            "mempalace_sessions", "mempalace_commits", "mempalace_team_share",
            "mempalace_reflect", "mempalace_migrate"
        ]
    })))
}

// ---------------------------------------------------------------------------
// Memory endpoints
// ---------------------------------------------------------------------------

async fn list_memories_handler(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args = map_to_json_object(params);
    let result = invoke_tool(&state_guard, "mempalace_status", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn get_memory_handler(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = json!({ "id": id }).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_recall", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn save_memory_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_save", args).await;
    Ok(tool_result_to_response(result))
}

async fn delete_memory_handler(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = json!({ "id": id }).as_object().unwrap().clone();
    let _result = invoke_tool(&state_guard, "mempalace_governance_delete", args).await;
    Ok(Json(json!({ "deleted": id })))
}

// ---------------------------------------------------------------------------
// Search endpoint
// ---------------------------------------------------------------------------

async fn search_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_recall", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn smart_search_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_smart_search", args).await?;
    Ok(Json(text_content_to_json(result)))
}

// ---------------------------------------------------------------------------
// Observe endpoint
// ---------------------------------------------------------------------------

async fn observe_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_observe", args).await?;
    Ok(Json(text_content_to_json(result)))
}

// ---------------------------------------------------------------------------
// Enrich endpoint
// ---------------------------------------------------------------------------

async fn enrich_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_enrich", args).await?;
    Ok(Json(text_content_to_json(result)))
}

// ---------------------------------------------------------------------------
// Consolidate endpoint
// ---------------------------------------------------------------------------

async fn consolidate_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_consolidate", args).await?;
    Ok(Json(text_content_to_json(result)))
}

// ---------------------------------------------------------------------------
// Knowledge Graph endpoints
// ---------------------------------------------------------------------------

async fn kg_query_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_kg_query", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn kg_add_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_kg_add", args).await;
    Ok(tool_result_to_response(result))
}

async fn kg_invalidate_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_kg_invalidate", args).await;
    Ok(tool_result_to_response(result))
}

async fn kg_stats_handler(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args = json!({}).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_kg_stats", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn kg_timeline_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_kg_timeline", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn kg_traverse_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_traverse", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn graph_stats_handler(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args = json!({}).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_graph_stats", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn graph_search_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_graph_search", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn graph_expand_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_graph_expand", args).await?;
    Ok(Json(text_content_to_json(result)))
}

// ---------------------------------------------------------------------------
// Diary endpoints
// ---------------------------------------------------------------------------

async fn diary_read_handler(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(json!({ "params": params }))
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_diary_read", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn diary_write_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_diary_write", args).await;
    Ok(tool_result_to_response(result))
}

// ---------------------------------------------------------------------------
// Slots endpoints
// ---------------------------------------------------------------------------

async fn slots_list_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_slot_list", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn slot_get_handler(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = json!({ "slot_id": id }).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_slot_get", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn slot_create_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_slot_create", args).await;
    Ok(tool_result_to_response(result))
}

async fn slot_append_handler(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let mut args = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    args.insert("slot_id".to_string(), serde_json::Value::String(id));
    let result = invoke_tool(&state_guard, "mempalace_slot_append", args).await;
    Ok(tool_result_to_response(result))
}

async fn slot_replace_handler(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let mut args = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    args.insert("slot_id".to_string(), serde_json::Value::String(id));
    let result = invoke_tool(&state_guard, "mempalace_slot_replace", args).await;
    Ok(tool_result_to_response(result))
}

async fn slot_delete_handler(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = json!({ "slot_id": id }).as_object().unwrap().clone();
    let _result = invoke_tool(&state_guard, "mempalace_slot_delete", args).await;
    Ok(Json(json!({ "deleted": id })))
}

// ---------------------------------------------------------------------------
// Actions endpoints
// ---------------------------------------------------------------------------

async fn actions_list_handler(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(json!({ "params": params }))
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_frontier", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn action_create_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_action_create", args).await;
    Ok(tool_result_to_response(result))
}

async fn action_update_handler(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let mut args = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    args.insert("action_id".to_string(), serde_json::Value::String(id));
    let result = invoke_tool(&state_guard, "mempalace_action_update", args).await;
    Ok(tool_result_to_response(result))
}

// ---------------------------------------------------------------------------
// Sentinels endpoints
// ---------------------------------------------------------------------------

async fn sentinels_list_handler(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args = json!({}).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_sentinel_list", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn sentinel_create_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_sentinel_create", args).await;
    Ok(tool_result_to_response(result))
}

async fn sentinel_trigger_handler(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = json!({ "sentinel_id": id }).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_sentinel_trigger", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn sentinel_delete_handler(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = json!({ "sentinel_id": id }).as_object().unwrap().clone();
    let _result = invoke_tool(&state_guard, "mempalace_sentinel_delete", args).await;
    Ok(Json(json!({ "deleted": id })))
}

// ---------------------------------------------------------------------------
// Checkpoints endpoints
// ---------------------------------------------------------------------------

async fn checkpoints_list_handler(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args = json!({}).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_checkpoint_list", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn checkpoint_resolve_handler(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = json!({ "checkpoint_id": id }).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_checkpoint_resolve", args).await;
    Ok(tool_result_to_response(result))
}

async fn checkpoint_create_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_checkpoint", args).await;
    Ok(tool_result_to_response(result))
}

// ---------------------------------------------------------------------------
// Sessions endpoints
// ---------------------------------------------------------------------------

async fn sessions_handler(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(json!({ "params": params }))
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_sessions", args).await?;
    Ok(Json(text_content_to_json(result)))
}

// ---------------------------------------------------------------------------
// Commits endpoints
// ---------------------------------------------------------------------------

async fn commits_handler(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(json!({ "params": params }))
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_commits", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn commit_lookup_handler(
    State(state): State<SharedState>,
    Path(hash): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = json!({ "commit_hash": hash }).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_commit_lookup", args).await?;
    Ok(Json(text_content_to_json(result)))
}

// ---------------------------------------------------------------------------
// Team endpoints
// ---------------------------------------------------------------------------

async fn team_share_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_team_share", args).await;
    Ok(tool_result_to_response(result))
}

async fn team_feed_handler(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(json!({ "params": params }))
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_team_feed", args).await?;
    Ok(Json(text_content_to_json(result)))
}

// ---------------------------------------------------------------------------
// Reflect endpoint
// ---------------------------------------------------------------------------

async fn reflect_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_reflect", args).await?;
    Ok(Json(text_content_to_json(result)))
}

// ---------------------------------------------------------------------------
// Migrate endpoint
// ---------------------------------------------------------------------------

async fn migrate_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_migrate", args).await;
    Ok(tool_result_to_response(result))
}

// ---------------------------------------------------------------------------
// Additional tool endpoints
// ---------------------------------------------------------------------------

async fn status_handler(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args = json!({}).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_status", args).await?;
    Ok(Json(text_content_to_json(result)))
}

/// Live-graph viewer SPA shell. Served at `GET /viewer/`.
/// The HTML, CSS, and JS are embedded at compile time via `include_str!`
/// in `viewer::mod`, so the binary ships as a single file with no
/// external asset directory. The full live-graph SPA (force layout,
/// search, SSE live updates) is the G5 follow-up in REMAINING.md.
async fn viewer_handler() -> impl IntoResponse {
    Html(crate::viewer_html())
}

/// SPA stylesheet. Served at `GET /viewer/styles.css`.
async fn viewer_styles_handler() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "text/css; charset=utf-8")],
        crate::viewer_styles_css(),
    )
}

/// SPA logic. Served at `GET /viewer/app.js`.
async fn viewer_app_handler() -> impl IntoResponse {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        crate::viewer_app_js(),
    )
}

async fn context_build_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_context_build", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn timeline_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_timeline", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn patterns_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_patterns", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn audit_handler(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(json!({ "params": params }))
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_audit", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn relations_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_relations", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn profile_handler(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(json!({ "params": params }))
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_profile", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn skill_extract_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_skill_extract", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn retention_score_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_retention_score", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn access_stats_handler(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(json!({ "params": params }))
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_access_stats", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn vision_search_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_vision_search", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn sketch_create_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_sketch_create", args).await;
    Ok(tool_result_to_response(result))
}

async fn sketch_promote_handler(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let mut args = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    args.insert("sketch_id".to_string(), serde_json::Value::String(id));
    let result = invoke_tool(&state_guard, "mempalace_sketch_promote", args).await;
    Ok(tool_result_to_response(result))
}

async fn crystallize_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_crystallize", args).await;
    Ok(tool_result_to_response(result))
}

async fn diagnose_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_diagnose", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn facet_tag_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_facet_tag", args).await;
    Ok(tool_result_to_response(result))
}

async fn facet_query_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_facet_query", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn lesson_save_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_lesson_save", args).await;
    Ok(tool_result_to_response(result))
}

async fn lesson_recall_handler(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(json!({ "params": params }))
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_lesson_recall", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn insight_list_handler(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(json!({ "params": params }))
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_insight_list", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn working_memory_handler(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args = json!({}).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_working_memory", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn file_index_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_file_index", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn file_history_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_file_history", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn snapshot_create_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_snapshot_create", args).await;
    Ok(tool_result_to_response(result))
}

async fn heal_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_heal", args).await;
    Ok(tool_result_to_response(result))
}

async fn verify_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_verify", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn mesh_sync_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_mesh_sync", args).await?;
    Ok(Json(text_content_to_json(result)))
}

// ---------------------------------------------------------------------------
// P0: Session, Summarize, Forget, Remember, Liveness, Config-Flags
// ---------------------------------------------------------------------------

async fn session_start_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_session_start", args).await;
    Ok(tool_result_to_response(result))
}

async fn session_end_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_session_end", args).await;
    Ok(tool_result_to_response(result))
}

async fn summarize_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_summarize", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn forget_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_governance_delete", args).await;
    Ok(tool_result_to_response(result))
}

async fn remember_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_save", args).await;
    Ok(tool_result_to_response(result))
}

async fn liveness_handler() -> Json<serde_json::Value> {
    Json(json!({
        "status": "alive",
        "service": "mempalace-rest-api",
    }))
}

async fn config_flags_handler(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args = json!({}).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_config_flags", args).await?;
    Ok(Json(text_content_to_json(result)))
}

// ---------------------------------------------------------------------------
// P1: Semantic/Procedural Lists, Auto-Forget, Auto-Crystallize, Flow-Compress,
//      Replay, Snapshots, Governance, Sentinel, Insight, Lesson, Action, Lease, Routine
// ---------------------------------------------------------------------------

async fn semantic_list_handler(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(json!({ "params": params }))
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_semantic_list", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn semantic_list_post_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_semantic_list", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn procedural_list_handler(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(json!({ "params": params }))
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_procedural_list", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn procedural_list_post_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_procedural_list", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn auto_forget_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_auto_forget", args).await;
    Ok(tool_result_to_response(result))
}

async fn auto_crystallize_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_auto_crystallize", args).await;
    Ok(tool_result_to_response(result))
}

async fn flow_compress_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_flow_compress", args).await;
    Ok(tool_result_to_response(result))
}

async fn replay_sessions_handler(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(json!({ "params": params }))
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_replay_sessions", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn replay_load_handler(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(json!({ "params": params }))
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_replay_load", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn snapshots_handler(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args = json!({}).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_snapshot_list", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn snapshot_restore_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_snapshot_restore", args).await;
    Ok(tool_result_to_response(result))
}

async fn governance_bulk_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_governance_bulk", args).await;
    Ok(tool_result_to_response(result))
}

async fn sentinel_cancel_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_sentinel_cancel", args).await;
    Ok(tool_result_to_response(result))
}

async fn sentinel_check_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_sentinel_check", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn insight_search_handler(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(json!({ "params": params }))
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_insight_search", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn lesson_search_handler(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(json!({ "params": params }))
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_lesson_search", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn lesson_list_handler(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args = json!({}).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_lesson_list", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn lesson_strengthen_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_lesson_strengthen", args).await;
    Ok(tool_result_to_response(result))
}

async fn action_edge_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_action_edge", args).await;
    Ok(tool_result_to_response(result))
}

async fn lease_acquire_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let mut args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    args.insert("operation".to_string(), serde_json::Value::String("acquire".to_string()));
    let result = invoke_tool(&state_guard, "mempalace_lease", args).await;
    Ok(tool_result_to_response(result))
}

async fn lease_release_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let mut args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    args.insert("operation".to_string(), serde_json::Value::String("release".to_string()));
    let result = invoke_tool(&state_guard, "mempalace_lease", args).await;
    Ok(tool_result_to_response(result))
}

async fn lease_renew_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let mut args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    args.insert("operation".to_string(), serde_json::Value::String("renew".to_string()));
    let result = invoke_tool(&state_guard, "mempalace_lease", args).await;
    Ok(tool_result_to_response(result))
}

async fn routine_create_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_routine_create", args).await;
    Ok(tool_result_to_response(result))
}

async fn routine_list_handler(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args = json!({}).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_routine_list", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn routine_status_handler(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = json!({ "routine_id": id }).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_routine_status", args).await?;
    Ok(Json(text_content_to_json(result)))
}

// ---------------------------------------------------------------------------
// P2: Team Profile, Mesh, Action, Cascade, Rules, Evolve, Disk-Size,
//      Sketch, Crystal, Facet, Branch, Vision
// ---------------------------------------------------------------------------

async fn team_profile_handler(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args = json!({}).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_team_profile", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn mesh_register_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_mesh_register", args).await;
    Ok(tool_result_to_response(result))
}

async fn mesh_list_handler(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args = json!({}).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_mesh_list", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn mesh_export_handler(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args = json!({}).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_mesh_export", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn mesh_receive_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_mesh_receive", args).await;
    Ok(tool_result_to_response(result))
}

async fn action_list_handler(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(json!({ "params": params }))
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_frontier", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn action_get_handler(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = json!({ "action_id": id }).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_action_get", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn cascade_update_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_cascade_update", args).await;
    Ok(tool_result_to_response(result))
}

async fn generate_rules_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_generate_rules", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn evolve_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_evolve", args).await;
    Ok(tool_result_to_response(result))
}

async fn disk_size_manager_handler(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let manager = state_guard.disk_size_manager.lock().await;
    Ok(Json(json!({
        "current_bytes": manager.current_bytes(),
        "max_bytes": manager.max_bytes(),
        "remaining_bytes": manager.remaining_bytes(),
        "is_over_quota": manager.is_over_quota(),
    })))
}

async fn sketch_add_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_sketch_add", args).await;
    Ok(tool_result_to_response(result))
}

async fn sketch_discard_handler(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = json!({ "sketch_id": id }).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_sketch_discard", args).await;
    Ok(tool_result_to_response(result))
}

async fn sketch_gc_handler(
    State(state): State<SharedState>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args = json!({}).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_sketch_gc", args).await;
    Ok(tool_result_to_response(result))
}

async fn crystal_list_handler(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args = json!({}).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_crystal_list", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn facet_get_handler(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(json!({ "params": params }))
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_facet_get", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn facet_stats_handler(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args = json!({}).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_facet_stats", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn facet_untag_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<axum::response::Response, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_facet_untag", args).await;
    Ok(tool_result_to_response(result))
}

async fn branch_detect_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_branch_detect", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn branch_sessions_handler(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(json!({ "params": params }))
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_branch_sessions", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn branch_worktrees_handler(
    State(state): State<SharedState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args = json!({}).as_object().unwrap().clone();
    let result = invoke_tool(&state_guard, "mempalace_branch_worktrees", args).await?;
    Ok(Json(text_content_to_json(result)))
}

async fn vision_embed_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    let args: JsonObject = serde_json::to_value(body)
        .map_err(|e| ApiError { status: StatusCode::BAD_REQUEST, message: e.to_string() })?
        .as_object()
        .unwrap()
        .clone();
    let result = invoke_tool(&state_guard, "mempalace_vision_embed", args).await?;
    Ok(Json(text_content_to_json(result)))
}

// ---------------------------------------------------------------------------
// SSE Transport (P13) - simplified placeholder
// ---------------------------------------------------------------------------

async fn sse_handler(
    State(_state): State<SharedState>,
) -> axum::response::Response {
    // SSE endpoint - returns a simple response for now
    // Real SSE would stream events using broadcast channel
    let body = "event: connected\ndata: {\"status\":\"ok\"}\n\n";
    axum::response::Response::builder()
        .header("Content-Type", "text/event-stream")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .body(body.into())
        .unwrap_or_else(|_| (StatusCode::OK, Json(json!({ "sse": "active" }))).into_response())
}

async fn mcp_handler(
    State(state): State<SharedState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let state_guard = state.lock().await;
    
    let tool_name = body.get("tool").and_then(|t| t.as_str()).unwrap_or("");
    let args = body.get("args").and_then(|a| a.as_object().cloned()).unwrap_or_default();
    
    let result = invoke_tool(&state_guard, tool_name, args).await?;
    
    Ok(Json(text_content_to_json(result)))
}

// ---------------------------------------------------------------------------
// Server build and run
// ---------------------------------------------------------------------------

fn build_router(state: SharedState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        // Health & info
        .route("/health", get(health_handler))
        .route("/healthz", get(healthz_handler))
        .route("/livez", get(livez_handler))
        .route("/tools", get(list_tools_handler))
        // Memories
        .route("/memories", get(list_memories_handler))
        .route("/memories", post(save_memory_handler))
        .route("/memories/{id}", get(get_memory_handler))
        .route("/memories/{id}", post(delete_memory_handler))
        // Search
        .route("/search", post(search_handler))
        .route("/smart_search", post(smart_search_handler))
        // Observe & Enrich
        .route("/observe", post(observe_handler))
        .route("/enrich", post(enrich_handler))
        // Consolidate
        .route("/consolidate", post(consolidate_handler))
        // Knowledge Graph
        .route("/kg/query", post(kg_query_handler))
        .route("/kg/add", post(kg_add_handler))
        .route("/kg/invalidate", post(kg_invalidate_handler))
        .route("/kg/stats", get(kg_stats_handler))
        .route("/kg/timeline", post(kg_timeline_handler))
        .route("/kg/traverse", post(kg_traverse_handler))
        .route("/graph/stats", get(graph_stats_handler))
        .route("/graph/search", post(graph_search_handler))
        .route("/graph/expand", post(graph_expand_handler))
        // Diary
        .route("/diary/read", get(diary_read_handler))
        .route("/diary/write", post(diary_write_handler))
        // Slots
        .route("/slots", post(slot_create_handler))
        .route("/slots", get(slots_list_handler))
        .route("/slots/{id}", get(slot_get_handler))
        .route("/slots/{id}", post(slot_delete_handler))
        .route("/slots/{id}/append", post(slot_append_handler))
        .route("/slots/{id}/replace", post(slot_replace_handler))
        // Actions
        .route("/actions", get(actions_list_handler))
        .route("/actions", post(action_create_handler))
        .route("/actions/{id}", post(action_update_handler))
        // Sentinels
        .route("/sentinels", get(sentinels_list_handler))
        .route("/sentinels", post(sentinel_create_handler))
        .route("/sentinels/{id}", post(sentinel_delete_handler))
        .route("/sentinels/{id}/trigger", post(sentinel_trigger_handler))
        // Checkpoints
        .route("/checkpoints", get(checkpoints_list_handler))
        .route("/checkpoints", post(checkpoint_create_handler))
        .route("/checkpoints/{id}/resolve", post(checkpoint_resolve_handler))
        // Sessions
        .route("/sessions", get(sessions_handler))
        // Commits
        .route("/commits", get(commits_handler))
        .route("/commits/{hash}", get(commit_lookup_handler))
        // Team
        .route("/team/share", post(team_share_handler))
        .route("/team/feed", get(team_feed_handler))
        // Reflect & Migrate
        .route("/reflect", post(reflect_handler))
        .route("/migrate", post(migrate_handler))
        // Additional tools
        .route("/status", get(status_handler))
        .route("/context/build", post(context_build_handler))
        .route("/timeline", post(timeline_handler))
        .route("/patterns", post(patterns_handler))
        .route("/audit", get(audit_handler))
        .route("/relations", post(relations_handler))
        .route("/profile", get(profile_handler))
        .route("/skill/extract", post(skill_extract_handler))
        .route("/retention/score", post(retention_score_handler))
        .route("/access/stats", get(access_stats_handler))
        .route("/vision/search", post(vision_search_handler))
        .route("/sketches", post(sketch_create_handler))
        .route("/sketches/{id}/promote", post(sketch_promote_handler))
        .route("/crystallize", post(crystallize_handler))
        .route("/diagnose", post(diagnose_handler))
        .route("/facet/tag", post(facet_tag_handler))
        .route("/facet/query", post(facet_query_handler))
        .route("/lessons/save", post(lesson_save_handler))
        .route("/lessons/recall", get(lesson_recall_handler))
        .route("/insights", get(insight_list_handler))
        .route("/working_memory", get(working_memory_handler))
        .route("/file/index", post(file_index_handler))
        .route("/file/history", post(file_history_handler))
        .route("/snapshot", post(snapshot_create_handler))
        .route("/heal", post(heal_handler))
        .route("/verify", post(verify_handler))
        .route("/mesh/sync", post(mesh_sync_handler))
        // P0: Session, Summarize, Forget, Remember, Liveness, Config-Flags
        .route("/session/start", post(session_start_handler))
        .route("/session/end", post(session_end_handler))
        .route("/summarize", post(summarize_handler))
        .route("/forget", post(forget_handler))
        .route("/remember", post(remember_handler))
        .route("/liveness", get(liveness_handler))
        .route("/config-flags", get(config_flags_handler))
        // P1: Semantic/Procedural Lists, Auto-Forget, Auto-Crystallize, Flow-Compress,
        //      Replay, Snapshots, Governance, Sentinel, Insight, Lesson, Action, Lease, Routine
        .route("/semantic-list", get(semantic_list_handler))
        .route("/semantic-list", post(semantic_list_post_handler))
        .route("/procedural-list", get(procedural_list_handler))
        .route("/procedural-list", post(procedural_list_post_handler))
        .route("/auto-forget", post(auto_forget_handler))
        .route("/auto-crystallize", post(auto_crystallize_handler))
        .route("/flow-compress", post(flow_compress_handler))
        .route("/replay/sessions", get(replay_sessions_handler))
        .route("/replay/load", get(replay_load_handler))
        .route("/snapshots", get(snapshots_handler))
        .route("/snapshot/restore", post(snapshot_restore_handler))
        .route("/governance/bulk", post(governance_bulk_handler))
        .route("/sentinel/cancel", post(sentinel_cancel_handler))
        .route("/sentinel/check", post(sentinel_check_handler))
        .route("/insight/search", get(insight_search_handler))
        .route("/lesson/search", get(lesson_search_handler))
        .route("/lesson/list", get(lesson_list_handler))
        .route("/lesson/strengthen", post(lesson_strengthen_handler))
        .route("/action/edge", post(action_edge_handler))
        .route("/lease/acquire", post(lease_acquire_handler))
        .route("/lease/release", post(lease_release_handler))
        .route("/lease/renew", post(lease_renew_handler))
        .route("/routine/create", post(routine_create_handler))
        .route("/routine/list", get(routine_list_handler))
        .route("/routine/status/{id}", get(routine_status_handler))
        // P2: Team Profile, Mesh, Action, Cascade, Rules, Evolve, Disk-Size,
        //      Sketch, Crystal, Facet, Branch, Vision
        .route("/team/profile", get(team_profile_handler))
        .route("/mesh/register", post(mesh_register_handler))
        .route("/mesh/list", get(mesh_list_handler))
        .route("/mesh/export", get(mesh_export_handler))
        .route("/mesh/receive", post(mesh_receive_handler))
        .route("/action/list", get(action_list_handler))
        .route("/action/{id}", get(action_get_handler))
        .route("/cascade-update", post(cascade_update_handler))
        .route("/generate-rules", post(generate_rules_handler))
        .route("/evolve", post(evolve_handler))
        .route("/disk-size-manager", get(disk_size_manager_handler))
        .route("/sketch/add", post(sketch_add_handler))
        .route("/sketch/discard/{id}", get(sketch_discard_handler))
        .route("/sketch/gc", get(sketch_gc_handler))
        .route("/crystal/list", get(crystal_list_handler))
        .route("/facet/get", get(facet_get_handler))
        .route("/facet/stats", get(facet_stats_handler))
        .route("/facet/untag", post(facet_untag_handler))
        .route("/branch/detect", post(branch_detect_handler))
        .route("/branch/sessions", get(branch_sessions_handler))
        .route("/branch/worktrees", get(branch_worktrees_handler))
        .route("/vision/embed", post(vision_embed_handler))
        // Live-graph viewer SPA (REMAINING.md G5 follow-up).
        .route("/viewer", get(viewer_handler))
        .route("/viewer/", get(viewer_handler))
        .route("/viewer/app.js", get(viewer_app_handler))
        .route("/viewer/styles.css", get(viewer_styles_handler))
        // SSE Transport (P13)
        .route("/sse", get(sse_handler))
        .route("/mcp", post(mcp_handler))
        .layer(cors)
        .with_state(state)
}

/// Start the HTTP server on the specified port.
#[cfg(feature = "health")]
pub async fn run_http_server(
    app_state: Arc<AppState>,
    read_only: bool,
    port: u16,
    embedder: std::sync::Arc<dyn crate::embed::Embedder>,
) -> anyhow::Result<()> {
    crate::health::init_health_monitor(
        app_state.palace_path.clone(),
        Arc::clone(&embedder),
        100, // default worker limit
    );

    let disk_size_manager = DiskSizeManager::new(10 * 1024 * 1024 * 1024);
    let state = Arc::new(Mutex::new(HttpServerState {
        app_state,
        read_only,
        disk_size_manager: Arc::new(Mutex::new(disk_size_manager)),
        embedder,
    }));

    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("REST API server listening on http://{}", addr);

    let router = build_router(state);
    axum::serve(listener, router).await?;

    Ok(())
}

/// Start the HTTP server on the specified port (no-op health monitoring).
#[cfg(not(feature = "health"))]
pub async fn run_http_server(
    app_state: Arc<AppState>,
    read_only: bool,
    port: u16,
) -> anyhow::Result<()> {
    let disk_size_manager = DiskSizeManager::new(10 * 1024 * 1024 * 1024);
    let state = Arc::new(Mutex::new(HttpServerState {
        app_state,
        read_only,
        disk_size_manager: Arc::new(Mutex::new(disk_size_manager)),
    }));

    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("REST API server listening on http://{}", addr);

    let router = build_router(state);
    axum::serve(listener, router).await?;

    Ok(())
}

/// Get the port from environment variable or default to 3111.
pub fn get_http_port() -> u16 {
    std::env::var("MEMPALACE_HTTP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3111)
}