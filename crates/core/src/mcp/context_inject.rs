//! Context injection for MemPalace standalone mode.
//!
//! When MEMPALACE_INJECT_CONTEXT or MEMPALACE_INJECT_CONTEXT is true,
//! after session_start inject recalled memories into context by calling
//! hybrid_search and formatting results as markdown.

use std::sync::Arc;

use crate::mcp_server::AppState;
use crate::searcher;

/// Environment flags for context injection.
const ENV_INJECT_FLAGS: &[&str] = &["MEMPALACE_INJECT_CONTEXT", "MEMPALACE_INJECT_CONTEXT"];

/// Returns true if context injection is enabled via env vars.
pub fn is_context_injection_enabled() -> bool {
    for flag in ENV_INJECT_FLAGS {
        if let Ok(val) = std::env::var(flag) {
            if val == "1" || val.to_lowercase() == "true" {
                return true;
            }
        }
    }
    false
}

/// Inject context after session_start by calling hybrid_search and
/// formatting results as markdown written to stdout.
///
/// Returns the context string that should be prepended to the prompt.
pub fn inject_session_context(
    session_id: &str,
    palace_path: &std::path::Path,
    project: Option<&str>,
) -> String {
    if !is_context_injection_enabled() {
        return String::new();
    }

    // Build a query from session context - recent memories for this session/project
    let query = project
        .map(|p| format!("recent session context project {}", p))
        .unwrap_or_else(|| format!("recent session {} context", session_id));

    // Run async search in a blocking context
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(async {
        crate::searcher::search_memories_with_rerank(
            &query,
            palace_path,
            None,    // wing filter - none
            None,    // room filter - none
            5,       // limit
            None,    // embedding model
            false,   // no BM25 reranking
            Some(3), // max_per_session
            None,    // fusion mode
        )
        .await
    });

    match result {
        Ok(response) => format_context_as_markdown(&response),
        Err(e) => {
            eprintln!("[mempalace] context injection failed: {}", e);
            String::new()
        }
    }
}

/// Format search results as markdown context string.
fn format_context_as_markdown(response: &crate::searcher::SearchResponse) -> String {
    if response.results.is_empty() {
        return String::new();
    }

    let mut output = String::from("## Recent Context\n");
    for result in response.results.iter() {
        let preview = if result.text.len() > 200 {
            format!("{}...", &result.text[..200])
        } else {
            result.text.clone()
        };
        output.push_str(&format!("- {}\n", preview.replace('\n', " ")));
    }
    output
}

/// Inject context using the MCP app state (async version for use from async dispatch).
pub async fn inject_context_async(
    state: &Arc<AppState>,
    session_id: &str,
    project: Option<&str>,
) -> Result<String, String> {
    if !is_context_injection_enabled() {
        return Ok(String::new());
    }

    let query = project
        .map(|p| format!("recent session context project {}", p))
        .unwrap_or_else(|| format!("recent session {} context", session_id));

    let result = crate::searcher::search_memories_with_rerank(
        &query,
        &state.palace_path,
        None,
        None,
        5,
        None,
        false,
        Some(3),
        None,
    )
    .await
    .map_err(|e| e.to_string())?;

    Ok(format_context_as_markdown(&result))
}
