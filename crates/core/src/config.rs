use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

const DEFAULT_COLLECTION_NAME: &str = "mempalace_drawers";

fn expand_path(path: &str) -> PathBuf {
    if path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(&path[2..]);
        }
    }
    PathBuf::from(path)
}

fn default_palace_path() -> PathBuf {
    expand_path("~/.mempalace/palace")
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
    m.insert(
        "emotions".to_string(),
        vec![
            "scared", "afraid", "worried", "happy", "sad", "love", "hate", "feel", "cry", "tears",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
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
    m.insert(
        "memory".to_string(),
        vec![
            "memory", "remember", "forget", "recall", "archive", "palace", "store",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
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
        vec!["identity", "name", "who am i", "persona", "self"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
    );
    m.insert(
        "family".to_string(),
        vec![
            "family", "kids", "children", "daughter", "son", "parent", "mother", "father",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    );
    m.insert(
        "creative".to_string(),
        vec![
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            palace_path: default_palace_path(),
            collection_name: default_collection_name(),
            people_map: HashMap::new(),
            topic_wings: default_topic_wings(),
            hall_keywords: default_hall_keywords(),
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
                PathBuf::from(env_val)
            } else {
                file_config
                    .get("palace_path")
                    .and_then(|v| v.as_str())
                    .map(PathBuf::from)
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
                people_map: HashMap::new(),
                topic_wings,
                hall_keywords,
            })
        } else {
            Ok(Config::default())
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let config_path = Self::config_file_path()?;
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(config_path, content)?;
        Ok(())
    }

    pub fn init(&self) -> anyhow::Result<PathBuf> {
        let config_dir = Self::config_dir()?;
        std::fs::create_dir_all(&config_dir)?;
        let config_path = config_dir.join("config.json");
        if !config_path.exists() {
            let default_config = Config::default();
            let content = serde_json::to_string_pretty(&default_config)?;
            std::fs::write(&config_path, content)?;
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
            Ok(HashMap::new())
        }
    }

    pub fn save_people_map(&self, people_map: &HashMap<String, String>) -> anyhow::Result<PathBuf> {
        let config_dir = Self::config_dir()?;
        std::fs::create_dir_all(&config_dir)?;
        let people_map_path = config_dir.join("people_map.json");
        let content = serde_json::to_string_pretty(people_map)?;
        std::fs::write(&people_map_path, content)?;
        Ok(people_map_path)
    }

    fn config_dir() -> anyhow::Result<PathBuf> {
        Ok(
            directories::ProjectDirs::from("com", "mempalace", "mempalace")
                .map(|d| d.config_dir().to_path_buf())
                .unwrap_or_else(|| expand_path("~/.mempalace")),
        )
    }

    fn config_file_path() -> anyhow::Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.json"))
    }
}
