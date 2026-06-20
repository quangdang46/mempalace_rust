//! Shared helper for writing MCP server config into a JSON envelope.
//!
//! Many agents use a simple `{ mcpServers: { ... } }` structure.  This
//! module provides a single reusable writer that:
//!   - Reads existing JSON if present
//!   - Adds / updates the `server_name` entry under `wrapper_key`
//!   - Writes atomically (write to `.tmp` then `fs::rename`)
//
//! For agents that use a different JSON key (e.g. Zed uses `context_servers`)
//! the caller specifies `wrapper_key` explicitly.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use tracing::{info, warn};

use super::types::{ConnectOptions, ConnectResult};

/// The standard MCP server entry written for mempalace.
///
/// Env vars use `${VAR:-default}` expansion so the wired entry inherits
/// `MEMPALACE_URL / MEMPALACE_SECRET / MEMPALACE_TOOLS` from the user's shell
/// but never fails parse when a var is unset (matching the upstream behavior).
fn mempalace_mcp_block() -> serde_json::Value {
    serde_json::json!({
        "command": "mpr",
        "args": ["mcp"],
        "env": {
            "MEMPALACE_URL": "${MEMPALACE_URL:-http://localhost:3111}",
            "MEMPALACE_SECRET": "${MEMPALACE_SECRET:-}",
            "MEMPALACE_TOOLS": "${MEMPALACE_TOOLS:-all}",
        }
    })
}

/// Write (or update) a mempalace MCP server entry inside a JSON config file.
///
/// * `path`         — target config file (e.g. `~/.cline/mcp.json`)
/// * `server_name`  — key inside the wrapper object (e.g. `"mempalace"`)
/// * `wrapper_key`  — top-level key (default `"mcpServers"`; Zed uses `"context_servers"`)
pub fn write_mcp_config(path: &Path, server_name: &str, wrapper_key: &str) -> ConnectResult {
    let adapter = server_name.to_string();
    let config_path = path.to_path_buf();

    // Determine parent dir for creation
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            if let Err(e) = fs::create_dir_all(parent) {
                warn!(
                    "connect: could not create config directory {:?}: {}",
                    parent, e
                );
                return ConnectResult {
                    adapter,
                    config_path,
                    wrote: false,
                    note: Some(format!("cannot create config directory: {}", e)),
                };
            }
        }
    }

    // Read existing JSON or start empty
    let existing: serde_json::Value = if path.exists() {
        match fs::read_to_string(path) {
            Ok(raw) => {
                serde_json::from_str(&raw).unwrap_or(serde_json::Value::Object(Default::default()))
            }
            Err(e) => {
                warn!("connect: could not read existing config {:?}: {}", path, e);
                serde_json::Value::Object(Default::default())
            }
        }
    } else {
        serde_json::Value::Object(Default::default())
    };

    // Mutate: add / replace server entry under wrapper_key
    let mut obj = existing.as_object().cloned().unwrap_or_default();
    let servers = obj
        .entry(wrapper_key)
        .or_insert_with(|| serde_json::Value::Object(Default::default()));
    if let Some(obj_servers) = servers.as_object_mut() {
        obj_servers.insert(server_name.to_string(), mempalace_mcp_block());
    } else {
        // wrapper_key existed but was not an object — replace it
        *servers = serde_json::Value::Object(Default::default());
        if let Some(obj_servers) = servers.as_object_mut() {
            obj_servers.insert(server_name.to_string(), mempalace_mcp_block());
        }
    }

    // Atomic write: tmp file then rename
    let tmp_path = format!("{}.tmp-{}", path.display(), std::process::id());
    let json_bytes = match serde_json::to_vec_pretty(&obj) {
        Ok(bytes) => bytes,
        Err(e) => {
            warn!("connect: JSON serialization failed: {}", e);
            return ConnectResult {
                adapter,
                config_path,
                wrote: false,
                note: Some(format!("JSON serialization failed: {}", e)),
            };
        }
    };
    match fs::File::create(&tmp_path) {
        Ok(mut f) => {
            if let Err(e) = f.write_all(&json_bytes) {
                warn!("connect: failed to write tmp file: {}", e);
                let _ = fs::remove_file(&tmp_path);
                return ConnectResult {
                    adapter,
                    config_path,
                    wrote: false,
                    note: Some(format!("write failed: {}", e)),
                };
            }
        }
        Err(e) => {
            warn!("connect: could not create tmp file: {}", e);
            return ConnectResult {
                adapter,
                config_path,
                wrote: false,
                note: Some(format!("cannot create tmp file: {}", e)),
            };
        }
    }

    if let Err(e) = fs::rename(&tmp_path, path) {
        warn!("connect: atomic rename failed: {}", e);
        let _ = fs::remove_file(&tmp_path);
        return ConnectResult {
            adapter,
            config_path,
            wrote: false,
            note: Some(format!("atomic rename failed: {}", e)),
        };
    }

    info!("connect: wrote {} entry to {:?}", server_name, path);
    ConnectResult {
        adapter,
        config_path,
        wrote: true,
        note: None,
    }
}

/// Check whether the config file already contains a mempalace entry.
pub fn already_wired(path: &Path, server_name: &str) -> bool {
    if !path.exists() {
        return false;
    }
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let val: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let servers = val.get("mcpServers").or_else(|| val.get("context_servers"));
    let servers_obj = match servers.and_then(|s| s.as_object()) {
        Some(o) => o,
        None => return false,
    };
    servers_obj.contains_key(server_name)
}
