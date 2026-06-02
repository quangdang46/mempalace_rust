//! Google Gemini CLI adapter.
//!
//! Gemini CLI stores MCP servers in `~/.gemini/settings.json`.
//! Schema follows the standard MCP envelope `{ mcpServers: { ... } }`.

use std::path::PathBuf;

use crate::connect::json_mcp::write_mcp_config;
use crate::connect::types::{ConnectOptions, ConnectResult};
use crate::connect::ConnectAdapter;

pub struct GeminiCliAdapter;

impl ConnectAdapter for GeminiCliAdapter {
    fn name(&self) -> &'static str {
        "gemini-cli"
    }

    fn config_path(&self) -> PathBuf {
        dirs::home_dir()
            .map(|p| p.join(".gemini/settings.json"))
            .unwrap_or_else(|| PathBuf::from("~/.gemini/settings.json"))
    }

    fn detect(&self) -> bool {
        dirs::home_dir()
            .map(|p| p.join(".gemini").exists())
            .unwrap_or(false)
    }

    fn connect(&self, opts: &ConnectOptions) -> std::result::Result<ConnectResult, anyhow::Error> {
        let path = self.config_path();
        let result = write_mcp_config(&path, "mempalace", "mcpServers");
        if opts.dry_run {
            tracing::info!(
                "connect [dry-run] {} -> {:?} (wrote={})",
                self.name(),
                path,
                result.wrote
            );
        }
        Ok(result)
    }
}
