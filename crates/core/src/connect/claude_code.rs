//! Claude Code adapter.
//!
//! Claude Code reads MCP servers from `~/.claude/mcp_servers.json`.
//! Schema follows the standard MCP envelope `{ mcpServers: { ... } }`.

use std::path::PathBuf;

use crate::connect::json_mcp::write_mcp_config;
use crate::connect::types::{ConnectOptions, ConnectResult};
use crate::connect::ConnectAdapter;

pub struct ClaudeCodeAdapter;

impl ConnectAdapter for ClaudeCodeAdapter {
    fn name(&self) -> &'static str {
        "claude-code"
    }

    fn config_path(&self) -> PathBuf {
        dirs::home_dir()
            .map(|p| p.join(".claude/mcp_servers.json"))
            .unwrap_or_else(|| PathBuf::from("~/.claude/mcp_servers.json"))
    }

    fn detect(&self) -> bool {
        dirs::home_dir()
            .map(|p| p.join(".claude").exists())
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
