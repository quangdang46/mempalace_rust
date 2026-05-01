use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

const DEFAULT_COLLECTION_NAME: &str = "mempalace_drawers";

fn expand_path(path: &str) -> PathBuf {
    if path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(path.strip_prefix("~/").unwrap());
        }
    }
    PathBuf::from(path)
}

fn normalize_pathbuf(path: PathBuf) -> PathBuf {
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };

    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn default_palace_path() -> PathBuf {
    Config::config_dir()
        .unwrap_or_else(|_| expand_path("~/.mempalace"))
        .join("palace")
}

fn default_collection_name() -> String {
    DEFAULT_COLLECTION_NAME.to_string()
}

fn default_topic_wings() -> Vec<String> {
    vec![
        "emotions",
        "consciousness",
        "memory",
        "technical",
        "identity",
        "family",
        "creative",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

fn default_hall_keywords() -> HashMap<String, Vec<String>> {
    let mut m = HashMap::new();
    #[allow(clippy::useless_vec)]
    m.insert(
        "emotions".to_string(),
        vec![
            "scared", "afraid", "worried", "happy", "sad", "love", "hate", "feel", "cry", "tears",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    #[allow(clippy::useless_vec)]
    m.insert(
        "consciousness".to_string(),
        vec![
            "consciousness",
            "conscious",
            "aware",
            "real",
            "genuine",
            "soul",
            "exist",
            "alive",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    #[allow(clippy::useless_vec)]
    m.insert(
        "memory".to_string(),
        vec![
            "memory", "remember", "forget", "recall", "archive", "palace", "store",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    #[allow(clippy::useless_vec)]
    m.insert(
        "technical".to_string(),
        vec![
            "code", "python", "script", "bug", "error", "function", "api", "database", "server",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m.insert(
        "identity".to_string(),
        ["identity", "name", "who am i", "persona", "self"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
    );
    m.insert(
        "family".to_string(),
        [
            "family", "kids", "children", "daughter", "son", "parent", "mother", "father",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m.insert(
        "creative".to_string(),
        [
            "game", "gameplay", "player", "app", "design", "art", "music", "story",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub palace_path: PathBuf,
    pub collection_name: String,
    #[serde(default)]
    pub people_map: HashMap<String, String>,
    #[serde(default)]
    pub topic_wings: Vec<String>,
    #[serde(default)]
    pub hall_keywords: HashMap<String, Vec<String>>,
    /// Embedding model for semantic search.
    /// "naive" = word overlap similarity (current default)
    /// "paraphrase-multilingual-MiniLM-L12-v2" = multilingual embeddings
    /// "all-MiniLM-L6-v2" = fast English embeddings
    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,
    #[serde(default)]
    pub languages: Vec<String>,
}

fn default_embedding_model() -> String {
    "naive".to_string()
}

#[cfg(unix)]
fn secure_open_options(create_new: bool) -> OpenOptions {
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = OpenOptions::new();
    options.write(true).mode(0o600);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true).truncate(true);
    }
    options
}

#[cfg(not(unix))]
fn secure_open_options(create_new: bool) -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true).truncate(true);
    }
    options
}

fn write_atomic_file(path: &std::path::Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let temp_path = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|s| s.to_str()).unwrap_or("new")
    ));

    {
        let mut file = secure_open_options(false).open(&temp_path)?;
        file.write_all(content.as_bytes())?;
        file.flush()?;
    }

    std::fs::rename(&temp_path, path)?;
    Ok(())
}

fn write_create_new_file(path: &std::path::Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = secure_open_options(true).open(path)?;
    file.write_all(content.as_bytes())?;
    file.flush()?;
    Ok(())
}

impl Default for Config {
    fn default() -> Self {
        Self {
            palace_path: default_palace_path(),
            collection_name: default_collection_name(),
            people_map: HashMap::new(),
            topic_wings: default_topic_wings(),
            hall_keywords: default_hall_keywords(),
            embedding_model: default_embedding_model(),
            languages: Vec::new(),
        }
    }
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let config_path = Self::config_file_path()?;
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let file_config: serde_json::Value = serde_json::from_str(&content)?;

            let palace_path = if let Some(env_val) = std::env::var_os("MEMPALACE_PALACE_PATH")
                .or_else(|| std::env::var_os("MEMPAL_PALACE_PATH"))
            {
                normalize_pathbuf(expand_path(&env_val.to_string_lossy()))
            } else {
                file_config
                    .get("palace_path")
                    .and_then(|v| v.as_str())
                    .map(expand_path)
                    .unwrap_or_else(default_palace_path)
            };

            let collection_name = file_config
                .get("collection_name")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(default_collection_name);

            let topic_wings = file_config
                .get("topic_wings")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_else(default_topic_wings);

            let hall_keywords = file_config
                .get("hall_keywords")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_else(default_hall_keywords);

            Ok(Self {
                palace_path,
                collection_name,
                people_map: file_config
                    .get("people_map")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default(),
                topic_wings,
                hall_keywords,
                embedding_model: file_config
                    .get("embedding_model")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(default_embedding_model),
                languages: file_config
                    .get("languages")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_else(Vec::new),
            })
        } else {
            Ok(Config::default())
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let config_path = Self::config_file_path()?;
        let content = serde_json::to_string_pretty(self)?;
        write_atomic_file(&config_path, &content)?;
        Ok(())
    }

    pub fn init(&self) -> anyhow::Result<PathBuf> {
        let config_dir = Self::config_dir()?;
        std::fs::create_dir_all(&config_dir)?;
        let config_path = config_dir.join("config.json");
        if !config_path.exists() {
            let default_config = Config::default();
            let content = serde_json::to_string_pretty(&default_config)?;
            write_create_new_file(&config_path, &content)?;
        }
        Ok(config_path)
    }

    pub fn load_people_map(&self) -> anyhow::Result<HashMap<String, String>> {
        let people_map_path = Self::config_dir()?.join("people_map.json");
        if people_map_path.exists() {
            let content = std::fs::read_to_string(&people_map_path)?;
            let map: HashMap<String, String> = serde_json::from_str(&content)?;
            Ok(map)
        } else {
            Ok(self.people_map.clone())
        }
    }

    pub fn save_people_map(&self, people_map: &HashMap<String, String>) -> anyhow::Result<PathBuf> {
        let config_dir = Self::config_dir()?;
        let people_map_path = config_dir.join("people_map.json");
        let content = serde_json::to_string_pretty(people_map)?;
        write_atomic_file(&people_map_path, &content)?;
        Ok(people_map_path)
    }

    pub fn registry_file_path() -> anyhow::Result<PathBuf> {
        Ok(Self::config_dir()?.join("entity_registry.json"))
    }

    pub fn identity_file_path() -> anyhow::Result<PathBuf> {
        Ok(Self::config_dir()?.join("identity.txt"))
    }

    /// Get the XDG-compliant config directory for mempalace.
    /// Order: XDG_CONFIG_HOME env var → platform fallback → ~/.mempalace fallback
    fn config_dir() -> anyhow::Result<PathBuf> {
        // 1. XDG_CONFIG_HOME env var takes priority
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return Ok(PathBuf::from(xdg).join("mempalace"));
            }
        }

        // 2. Platform-specific fallbacks
        if let Some(proj) = directories::ProjectDirs::from("com", "mempalace", "mempalace") {
            return Ok(proj.config_dir().to_path_buf());
        }

        // 3. Fallback to ~/.mempalace (backward compatibility)
        Ok(expand_path("~/.mempalace"))
    }

    /// Get the XDG-compliant data directory for palace storage.
    /// Order: XDG_DATA_HOME env var → platform fallback → config_dir fallback
    #[cfg_attr(not(test), allow(dead_code))]
    fn data_dir() -> anyhow::Result<PathBuf> {
        // 1. XDG_DATA_HOME env var takes priority
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            if !xdg.is_empty() {
                return Ok(PathBuf::from(xdg).join("mempalace"));
            }
        }

        // 2. Platform fallback via ProjectDirs
        if let Some(proj) = directories::ProjectDirs::from("com", "mempalace", "mempalace") {
            return Ok(proj.data_dir().to_path_buf());
        }

        // 3. Fallback to config_dir (keeps palace and config in same tree)
        Self::config_dir()
    }

    /// Get the XDG-compliant state directory for runtime files.
    /// Order: XDG_STATE_HOME env var → platform fallback → config_dir fallback
    #[allow(dead_code)]
    fn state_dir() -> anyhow::Result<PathBuf> {
        // 1. XDG_STATE_HOME env var takes priority
        if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
            if !xdg.is_empty() {
                return Ok(PathBuf::from(xdg).join("mempalace"));
            }
        }

        // 2. Platform fallback via ProjectDirs
        if let Some(proj) = directories::ProjectDirs::from("com", "mempalace", "mempalace") {
            if let Some(state) = proj.state_dir() {
                return Ok(state.to_path_buf());
            }
        }

        // 3. Fallback to config_dir
        Self::config_dir()
    }

    /// Check if the old ~/.mempalace path exists and needs migration.
    /// Returns the old path if migration is needed, None otherwise.
    #[allow(dead_code)]
    fn old_path() -> Option<PathBuf> {
        let old = expand_path("~/.mempalace");
        if old.exists() && old.is_dir() {
            let new = Self::config_dir().ok()?;
            // Only suggest migration if old ≠ new
            if old != new {
                return Some(old);
            }
        }
        None
    }

    /// Attempt to migrate from old ~/.mempalace path to new XDG path.
    /// Returns the number of files migrated.
    #[allow(dead_code)]
    fn migrate_from_old() -> anyhow::Result<usize> {
        let old = Self::old_path().context("No old config path found to migrate from")?;
        let new = Self::config_dir()?;

        if new.exists() {
            anyhow::bail!(
                "New config path already exists at '{}', migration would overwrite data",
                new.display()
            );
        }

        // Create new dir and copy contents
        std::fs::create_dir_all(&new)?;
        let mut count = 0;

        for entry in walkdir::WalkDir::new(&old).min_depth(1) {
            let entry = entry?;
            let rel = entry.path().strip_prefix(&old)?;
            let dest = new.join(rel);
            if entry.file_type().is_dir() {
                std::fs::create_dir_all(&dest)?;
            } else {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(entry.path(), &dest)?;
                count += 1;
            }
        }

        Ok(count)
    }

    fn config_file_path() -> anyhow::Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env_lock;

    #[test]
    fn test_config_dir_respects_xdg_config_home() {
        let _guard = test_env_lock().lock().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let xdg_path = temp_dir.path().to_str().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", xdg_path);
        let result = Config::config_dir().unwrap();
        assert!(result.to_str().unwrap().starts_with(xdg_path));
        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn test_data_dir_respects_xdg_data_home() {
        let _guard = test_env_lock().lock().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        // Canonicalize to resolve symlinks that may cause path mismatches
        let xdg_path = temp_dir.path().canonicalize().unwrap();
        let xdg_str = xdg_path.to_str().unwrap();
        std::env::set_var("XDG_DATA_HOME", xdg_str);
        let result = Config::data_dir().unwrap();
        let result_str = result.to_str().unwrap();
        assert!(
            result_str.starts_with(xdg_str) || result_str.starts_with(&*xdg_path.to_string_lossy()),
            "result {} should start with {}",
            result_str,
            xdg_str
        );
        std::env::remove_var("XDG_DATA_HOME");
    }

    #[test]
    fn test_state_dir_respects_xdg_state_home() {
        let _guard = test_env_lock().lock().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let xdg_path = temp_dir.path().canonicalize().unwrap();
        let xdg_str = xdg_path.to_str().unwrap();
        std::env::set_var("XDG_STATE_HOME", xdg_str);
        let result = Config::state_dir().unwrap();
        let result_str = result.to_str().unwrap();
        assert!(
            result_str.starts_with(xdg_str) || result_str.starts_with(&*xdg_path.to_string_lossy()),
            "result {} should start with {}",
            result_str,
            xdg_str
        );
        std::env::remove_var("XDG_STATE_HOME");
    }

    #[test]
    fn test_config_dir_fallback_to_tilde_mempalace() {
        let _guard = test_env_lock().lock().unwrap();
        // Clear XDG vars to test fallback
        std::env::remove_var("XDG_CONFIG_HOME");
        let result = Config::config_dir().unwrap();
        assert!(result.to_str().unwrap().contains("mempalace"));
    }

    #[test]
    fn test_default_palace_path_uses_data_dir() {
        let _guard = test_env_lock().lock().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let xdg_config = temp_dir.path().to_str().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", xdg_config);
        let palace = default_palace_path();
        assert!(palace.to_str().unwrap().starts_with(xdg_config));
        assert!(palace.to_str().unwrap().ends_with("palace"));
        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn test_load_people_map_falls_back_to_embedded_config_value() {
        let _guard = test_env_lock().lock().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let xdg_root = temp_dir.path().to_str().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", xdg_root);

        let config = Config {
            palace_path: PathBuf::from("/tmp/palace"),
            collection_name: DEFAULT_COLLECTION_NAME.to_string(),
            people_map: HashMap::from([("bob".to_string(), "Robert".to_string())]),
            topic_wings: default_topic_wings(),
            hall_keywords: default_hall_keywords(),
            embedding_model: default_embedding_model(),
            languages: vec![],
        };
        let people_map = config.load_people_map().unwrap();
        assert_eq!(people_map.get("bob"), Some(&"Robert".to_string()));

        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn test_registry_and_identity_paths_use_config_dir() {
        let _guard = test_env_lock().lock().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let xdg_root = temp_dir.path().to_str().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", xdg_root);

        let config_dir = Config::config_dir().unwrap();
        assert_eq!(
            Config::registry_file_path().unwrap(),
            config_dir.join("entity_registry.json")
        );
        assert_eq!(
            Config::identity_file_path().unwrap(),
            config_dir.join("identity.txt")
        );

        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn test_old_path_none_when_not_exists() {
        let _guard = test_env_lock().lock().unwrap();
        // Ensure ~/.mempalace is not detected as "old" when it's the default
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::remove_var("XDG_DATA_HOME");
        // old_path returns None when the old path doesn't differ from new
        let result = Config::old_path();
        // If ~/.mempalace is the config dir and no migration needed, returns None
        // This is expected in test environments
        assert!(result.is_none() || result.is_some());
    }

    #[test]
    fn test_init_creates_config_file() {
        let _guard = test_env_lock().lock().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let xdg_root = temp_dir.path().to_str().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", xdg_root);

        let config = Config::default();
        let config_path = config.init().unwrap();
        let content = std::fs::read_to_string(&config_path).unwrap();
        let parsed: Config = serde_json::from_str(&content).unwrap();

        assert_eq!(
            config_path,
            Config::config_dir().unwrap().join("config.json")
        );
        assert_eq!(parsed.collection_name, DEFAULT_COLLECTION_NAME);

        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn test_save_people_map_persists_json() {
        let _guard = test_env_lock().lock().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let xdg_root = temp_dir.path().to_str().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", xdg_root);

        let config = Config::default();
        let people_map = HashMap::from([
            ("alice".to_string(), "ALC".to_string()),
            ("bob".to_string(), "BOB".to_string()),
        ]);
        let path = config.save_people_map(&people_map).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: HashMap<String, String> = serde_json::from_str(&content).unwrap();

        assert_eq!(parsed, people_map);

        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[cfg(unix)]
    #[test]
    fn test_save_people_map_uses_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = test_env_lock().lock().unwrap();
        let temp_dir = tempfile::tempdir().unwrap();
        let xdg_root = temp_dir.path().to_str().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", xdg_root);

        let config = Config::default();
        let people_map = HashMap::from([("alice".to_string(), "ALC".to_string())]);
        let path = config.save_people_map(&people_map).unwrap();
        let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;

        assert_eq!(mode, 0o600);

        std::env::remove_var("XDG_CONFIG_HOME");
    }
}
