// Plugin discovery — scans filesystem for manifest.json files and
// manages enable/disable state for mempalace plugins.
//
// Searches two locations:
//   - ~/.mempalace/plugins/*/manifest.json  (user-level)
//   - ./plugins/*/manifest.json            (project-level)
//
// All plugins start with enabled=false; use enable()/disable() to toggle.

pub mod skills;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// Plugin manifest — deserialized from manifest.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub hooks: Vec<String>,
    pub skills: Vec<String>,
}

/// A discovered plugin with manifest, filesystem path, and runtime state.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub manifest: PluginManifest,
    pub path: PathBuf,
    pub enabled: bool,
}

/// Errors that can occur during plugin operations.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("plugin not found: {0}")]
    NotFound(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// Module-level state — lazily initialised on first use
// ---------------------------------------------------------------------------

static PLUGIN_STATE: std::sync::OnceLock<RwLock<HashMap<String, bool>>> =
    std::sync::OnceLock::new();

fn plugin_state() -> &'static RwLock<HashMap<String, bool>> {
    PLUGIN_STATE.get_or_init(|| RwLock::new(HashMap::new()))
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Discover all plugins by scanning both user and project plugin directories.
///
/// Each plugin starts with enabled=false; call enable() to activate.
pub fn discover() -> Vec<Plugin> {
    let mut plugins = Vec::new();

    // User-level plugins: ~/.mempalace/plugins/*/manifest.json
    if let Some(user_dir) = dirs::home_dir().map(|p| p.join(".mempalace/plugins")) {
        scan_dir(&user_dir, &mut plugins);
    }

    // Project-level plugins: ./plugins/*/manifest.json
    scan_dir(Path::new("plugins"), &mut plugins);

    plugins
}

/// Scan a single directory for plugin subdirectories containing manifest.json.
fn scan_dir(dir: &Path, plugins: &mut Vec<Plugin>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.filter_map(Result::ok) {
        let subdir = entry.path();
        if !entry.file_type().is_ok_and(|ft| ft.is_dir()) {
            continue;
        }
        let manifest_path = subdir.join("manifest.json");
        if manifest_path.is_file() {
            if let Some(plugin) = load_plugin(&subdir, &manifest_path) {
                plugins.push(plugin);
            }
        }
    }
}

/// Load a single plugin from its manifest.json path.
fn load_plugin(dir: &Path, manifest_path: &Path) -> Option<Plugin> {
    let content = fs::read_to_string(manifest_path).ok()?;
    let manifest: PluginManifest = serde_json::from_str(&content).ok()?;
    let name = manifest.name.clone();

    // Initialise enabled=false in module state
    let mut state = plugin_state().write().expect("plugin state lock poisoned");
    state.entry(name).or_insert(false);

    Some(Plugin {
        manifest,
        path: dir.to_path_buf(),
        enabled: false,
    })
}

/// Enable a plugin by name.  Returns Ok(()) on success.
pub fn enable(plugin_name: &str) -> Result<(), PluginError> {
    let plugin = discover()
        .into_iter()
        .find(|p| p.manifest.name == plugin_name)
        .ok_or_else(|| PluginError::NotFound(plugin_name.to_string()))?;

    let mut state = plugin_state()
        .write()
        .expect("plugin state lock poisoned");
    state.insert(plugin_name.to_string(), true);
    let _ = plugin;
    Ok(())
}

/// Disable a plugin by name.  Returns Ok(()) on success.
pub fn disable(plugin_name: &str) -> Result<(), PluginError> {
    let _ = discover()
        .into_iter()
        .find(|p| p.manifest.name == plugin_name)
        .ok_or_else(|| PluginError::NotFound(plugin_name.to_string()))?;

    let mut state = plugin_state()
        .write()
        .expect("plugin state lock poisoned");
    state.insert(plugin_name.to_string(), false);
    Ok(())
}

/// List all discovered plugins as (name, version, enabled) tuples.
pub fn list() -> Vec<(String, String, bool)> {
    let state = plugin_state()
        .read()
        .expect("plugin state lock poisoned");

    discover()
        .into_iter()
        .map(|p| {
            let enabled = state.get(&p.manifest.name).copied().unwrap_or(false);
            (p.manifest.name, p.manifest.version, enabled)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_manifest_parse_minimal() {
        let json = r#"{"name":"test","version":"1.0","hooks":[],"skills":[]}"#;
        let m: PluginManifest = serde_json::from_str(json).expect("must parse minimal manifest");
        assert_eq!(m.name, "test");
        assert_eq!(m.version, "1.0");
        assert!(m.hooks.is_empty());
        assert!(m.skills.is_empty());
    }

    #[test]
    fn test_plugin_enabled_state() {
        // Create a real temp plugin so discover() finds it
        let tmp = TempDir::new().expect("temp dir");
        // discover() looks for ./plugins/<name>/manifest.json
        let plugin_dir = tmp.path().join("plugins/my_test_plugin");
        fs::create_dir_all(&plugin_dir).expect("create plugin dir");
        let manifest = r#"{"name":"my_test_plugin","version":"1.0.0","hooks":[],"skills":[]}"#;
        fs::write(plugin_dir.join("manifest.json"), manifest).expect("write manifest");

        // Override CWD so discover() picks up the temp plugin
        let original_cwd = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(tmp.path()).expect("set cwd");

        // Discover the plugin (initialises state to enabled=false)
        let discovered = discover();
        assert!(!discovered.is_empty(), "plugin must be discovered from temp dir");

        // Enable then disable
        enable("my_test_plugin").expect("enable must succeed");
        disable("my_test_plugin").expect("disable must succeed");

        // Verify final state is false
        let state = plugin_state().read().expect("plugin state lock poisoned");
        let enabled = state.get("my_test_plugin").copied().expect("key must exist");
        assert!(!enabled, "plugin should be disabled after disable()");

        // Restore cwd
        std::env::set_current_dir(original_cwd).ok();
    }

    #[test]
    fn test_list_returns_tuples() {
        let result = list();
        // Verify the return type is Vec<(String, String, bool)>
        for (name, version, enabled) in &result {
            let _: &str = name;
            let _: &str = version;
            let _: &bool = enabled;
        }
    }
}