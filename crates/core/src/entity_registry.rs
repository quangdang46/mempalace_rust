use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityEntry {
    pub source: String,
    pub contexts: Vec<String>,
    pub aliases: Vec<String>,
    pub relationship: String,
    pub confidence: f64,
    #[serde(default)]
    pub canonical: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryData {
    pub version: usize,
    pub mode: String,
    #[serde(default)]
    pub people: HashMap<String, EntityEntry>,
    #[serde(default)]
    pub projects: Vec<String>,
    #[serde(default)]
    pub ambiguous_flags: Vec<String>,
    #[serde(default)]
    pub wiki_cache: HashMap<String, serde_json::Value>,
    /// Entities explicitly rejected by the user during init or review.
    /// These are permanently ignored in future entity detection.
    #[serde(default)]
    pub rejected_entities: Vec<String>,
}

impl Default for RegistryData {
    fn default() -> Self {
        Self {
            version: 1,
            mode: "personal".to_string(),
            people: HashMap::new(),
            projects: Vec::new(),
            ambiguous_flags: Vec::new(),
            wiki_cache: HashMap::new(),
            rejected_entities: Vec::new(),
        }
    }
}

pub struct EntityRegistry {
    data: RegistryData,
    path: std::path::PathBuf,
}

#[derive(Debug, Clone)]
pub struct LookupResult {
    pub entity_type: String,
    pub confidence: f64,
    pub source: String,
    pub name: String,
    pub needs_disambiguation: bool,
}

static COMMON_ENGLISH_WORDS: &[&str] = &[
    "ever",
    "grace",
    "will",
    "bill",
    "mark",
    "april",
    "may",
    "june",
    "joy",
    "hope",
    "faith",
    "chance",
    "chase",
    "hunter",
    "dash",
    "flash",
    "star",
    "sky",
    "river",
    "brook",
    "lane",
    "art",
    "clay",
    "gil",
    "nat",
    "max",
    "rex",
    "ray",
    "jay",
    "rose",
    "violet",
    "lily",
    "ivy",
    "ash",
    "reed",
    "sage",
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
    "january",
    "february",
    "march",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

impl EntityRegistry {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            let content = std::fs::read_to_string(path)?;
            let data: RegistryData = serde_json::from_str(&content).unwrap_or_default();
            return Ok(Self {
                data,
                path: path.to_path_buf(),
            });
        }
        Ok(Self {
            data: RegistryData::default(),
            path: path.to_path_buf(),
        })
    }

    pub fn save(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(&self.data)?;
        std::fs::write(&self.path, content)?;
        Ok(())
    }

    pub fn seed(
        &mut self,
        mode: &str,
        people: Vec<(&str, &str, &str)>,
        projects: Vec<&str>,
        aliases: Option<HashMap<&str, &str>>,
    ) -> anyhow::Result<()> {
        self.data.mode = mode.to_string();
        self.data.projects = projects.iter().map(|s| s.to_string()).collect();

        let aliases = aliases.unwrap_or_default();
        let reverse_aliases: HashMap<_, _> = aliases
            .iter()
            .map(|(k, v)| (v.to_string(), k.to_string()))
            .collect();

        for (name, context, relationship) in people {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            let canonical = reverse_aliases
                .get(name)
                .map(|s| s.as_str())
                .unwrap_or(name);

            self.data.people.insert(
                name.to_string(),
                EntityEntry {
                    source: "onboarding".to_string(),
                    contexts: vec![context.to_string()],
                    aliases: if canonical != name {
                        vec![canonical.to_string()]
                    } else {
                        vec![]
                    },
                    relationship: relationship.to_string(),
                    confidence: 1.0,
                    canonical: if canonical != name {
                        Some(canonical.to_string())
                    } else {
                        None
                    },
                },
            );

            if let Some(alias) = reverse_aliases.get(name) {
                self.data.people.insert(
                    alias.to_string(),
                    EntityEntry {
                        source: "onboarding".to_string(),
                        contexts: vec![context.to_string()],
                        aliases: vec![name.to_string()],
                        relationship: relationship.to_string(),
                        confidence: 1.0,
                        canonical: Some(name.to_string()),
                    },
                );
            }
        }

        let ambiguous: Vec<String> = self
            .data
            .people
            .keys()
            .filter(|name| COMMON_ENGLISH_WORDS.contains(&name.to_lowercase().as_str()))
            .map(|name| name.to_lowercase())
            .collect();
        self.data.ambiguous_flags = ambiguous;

        self.save()?;
        Ok(())
    }

    pub fn lookup(&self, word: &str, context: &str) -> LookupResult {
        let word_lower = word.to_lowercase();

        for (canonical, info) in &self.data.people {
            if word_lower == canonical.to_lowercase()
                || info.aliases.iter().any(|a| a.to_lowercase() == word_lower)
            {
                if self.data.ambiguous_flags.contains(&word_lower) && !context.is_empty() {
                    if let Some(resolved) = self.disambiguate(word, context, info) {
                        return resolved;
                    }
                }
                return LookupResult {
                    entity_type: "person".to_string(),
                    confidence: info.confidence,
                    source: info.source.clone(),
                    name: canonical.clone(),
                    needs_disambiguation: false,
                };
            }
        }

        for proj in &self.data.projects {
            if word_lower == proj.to_lowercase() {
                return LookupResult {
                    entity_type: "project".to_string(),
                    confidence: 1.0,
                    source: "onboarding".to_string(),
                    name: proj.clone(),
                    needs_disambiguation: false,
                };
            }
        }

        LookupResult {
            entity_type: "unknown".to_string(),
            confidence: 0.0,
            source: "none".to_string(),
            name: word.to_string(),
            needs_disambiguation: false,
        }
    }

    fn disambiguate(
        &self,
        word: &str,
        context: &str,
        _person_info: &EntityEntry,
    ) -> Option<LookupResult> {
        let _word_lower = word.to_lowercase();
        let ctx_lower = context.to_lowercase();

        let person_indicators = ["said", "told", "asked", "was", "is", "called"];
        let concept_indicators = [
            r"have you",
            r"if you",
            r"since",
            r"again",
            r"not ",
            r"more",
            r"would",
            r"could",
            r"will",
        ];

        let mut person_score = 0;
        let mut concept_score = 0;

        for indicator in &person_indicators {
            let pattern = format!(r"(?i)\b{}\b", indicator);
            if Regex::new(&pattern)
                .map(|re| re.is_match(&ctx_lower))
                .unwrap_or(false)
            {
                person_score += 1;
            }
        }

        for indicator in &concept_indicators {
            let pattern = format!(r"(?i){}", indicator);
            if Regex::new(&pattern)
                .map(|re| re.is_match(&ctx_lower))
                .unwrap_or(false)
            {
                concept_score += 1;
            }
        }

        if person_score > concept_score {
            Some(LookupResult {
                entity_type: "person".to_string(),
                confidence: (0.7 + person_score as f64 * 0.1).min(0.95),
                source: "context_patterns".to_string(),
                name: word.to_string(),
                needs_disambiguation: false,
            })
        } else if concept_score > person_score {
            Some(LookupResult {
                entity_type: "concept".to_string(),
                confidence: (0.7 + concept_score as f64 * 0.1).min(0.90),
                source: "context_disambiguated".to_string(),
                name: word.to_string(),
                needs_disambiguation: false,
            })
        } else {
            None
        }
    }

    pub fn extract_people_from_query(&self, query: &str) -> Vec<String> {
        let mut found = Vec::new();

        for (canonical, info) in &self.data.people {
            let names_to_check: Vec<&str> = std::iter::once(canonical.as_str())
                .chain(info.aliases.iter().map(|s| s.as_str()))
                .collect();

            for name in names_to_check {
                let pattern = format!(r"(?i)\b{}\b", regex::escape(name));
                if let Ok(re) = Regex::new(&pattern) {
                    if re.is_match(query) {
                        let name_lower = name.to_lowercase();
                        if self.data.ambiguous_flags.contains(&name_lower) {
                            if let Some(result) = self.disambiguate(name, query, info) {
                                if result.entity_type == "person" && !found.contains(canonical) {
                                    found.push(canonical.clone());
                                }
                            }
                        } else if !found.contains(canonical) {
                            found.push(canonical.clone());
                        }
                        break;
                    }
                }
            }
        }

        found
    }

    pub fn extract_unknown_candidates(&self, query: &str) -> Vec<String> {
        let mut unknown = Vec::new();
        let word_pattern = Regex::new(r"\b[A-Z][a-z]{2,15}\b").ok();

        if let Some(re) = word_pattern {
            let candidates: Vec<String> = re
                .find_iter(query)
                .map(|m| m.as_str().to_string())
                .collect();

            for word in candidates {
                let word_lower = word.to_lowercase();
                if COMMON_ENGLISH_WORDS.contains(&word_lower.as_str()) {
                    continue;
                }
                if self
                    .data
                    .projects
                    .iter()
                    .any(|p| p.to_lowercase() == word_lower)
                {
                    continue;
                }
                if self.data.people.contains_key(&word) {
                    continue;
                }
                unknown.push(word);
            }
        }

        unknown
    }

    pub fn summary(&self) -> String {
        let people_preview: Vec<_> = self
            .data
            .people
            .keys()
            .take(8)
            .map(|s| s.as_str())
            .collect::<Vec<_>>();
        let people_str = if self.data.people.len() > 8 {
            format!("{} (...)", people_preview.join(", "))
        } else {
            people_preview.join(", ")
        };

        format!(
            "Mode: {}\nPeople: {} ({})\nProjects: {}\nAmbiguous flags: {}",
            self.data.mode,
            self.data.people.len(),
            people_str,
            self.data.projects.join(", "),
            self.data.ambiguous_flags.join(", "),
        )
    }

    pub fn people_count(&self) -> usize {
        self.data.people.len()
    }

    pub fn projects_count(&self) -> usize {
        self.data.projects.len()
    }

    /// Add an entity to the rejected list so it won't be re-detected.
    pub fn reject_entity(&mut self, name: &str) {
        let lower = name.to_lowercase();
        if !self.data.rejected_entities.contains(&lower) {
            self.data.rejected_entities.push(lower);
        }
    }

    /// Check if an entity was previously rejected by the user.
    pub fn is_rejected(&self, name: &str) -> bool {
        self.data.rejected_entities.contains(&name.to_lowercase())
    }

    /// Get the list of all rejected entity names.
    pub fn get_rejected(&self) -> &[String] {
        &self.data.rejected_entities
    }

    /// Filter out rejected entities from a list of candidate names.
    pub fn filter_rejected(&self, names: &[String]) -> Vec<String> {
        names
            .iter()
            .filter(|n| !self.is_rejected(n))
            .cloned()
            .collect()
    }

    /// Reject multiple entities at once (e.g., from an interactive confirmation).
    pub fn reject_entities(&mut self, names: &[String]) {
        for name in names {
            self.reject_entity(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_seed_and_lookup() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("registry.json");

        let mut registry = EntityRegistry::load(&path).unwrap();
        registry
            .seed(
                "personal",
                vec![
                    ("Alice", "personal", "friend"),
                    ("Bob", "work", "colleague"),
                ],
                vec!["ProjectX"],
                None,
            )
            .unwrap();

        let result = registry.lookup("Alice", "");
        assert_eq!(result.entity_type, "person");
        assert_eq!(result.confidence, 1.0);

        let result = registry.lookup("Bob", "");
        assert_eq!(result.entity_type, "person");
        assert_eq!(result.source, "onboarding");

        let result = registry.lookup("ProjectX", "");
        assert_eq!(result.entity_type, "project");
    }

    #[test]
    fn test_extract_people_from_query() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("registry.json");

        let mut registry = EntityRegistry::load(&path).unwrap();
        registry
            .seed(
                "personal",
                vec![
                    ("Alice", "personal", ""),
                    ("Bob", "personal", ""),
                    ("Charlie", "personal", ""),
                ],
                vec![],
                None,
            )
            .unwrap();

        let found =
            registry.extract_people_from_query("What did Alice and Bob discuss with Charlie?");
        assert_eq!(found.len(), 3);
        assert!(found.contains(&"Alice".to_string()));
        assert!(found.contains(&"Bob".to_string()));
        assert!(found.contains(&"Charlie".to_string()));
    }

    #[test]
    fn test_extract_unknown_candidates() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("registry.json");

        let registry = EntityRegistry::load(&path).unwrap();
        let candidates =
            registry.extract_unknown_candidates("Alice talked to Bob about ProjectX and Eve");
        assert!(candidates.contains(&"Alice".to_string()));
        assert!(candidates.contains(&"Bob".to_string()));
        assert!(!candidates.contains(&"ProjectX".to_string()));
    }

    #[test]
    fn test_ambiguous_words() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("registry.json");

        let mut registry = EntityRegistry::load(&path).unwrap();
        registry
            .seed(
                "personal",
                vec![("May", "personal", "friend")],
                vec![],
                None,
            )
            .unwrap();

        let result = registry.lookup("May", "Have you seen May today?");
        assert_eq!(result.entity_type, "concept");

        let result = registry.lookup("May", "May said hello to me");
        assert_eq!(result.entity_type, "person");
    }
}
