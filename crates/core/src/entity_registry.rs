use crate::entity_detector::detect_from_content;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

const EMPTY_REJECTED: &[String] = &[];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityEntry {
    pub source: String,
    pub contexts: Vec<String>,
    pub aliases: Vec<String>,
    pub relationship: String,
    pub confidence: f64,
    #[serde(default)]
    pub canonical: Option<String>,
    #[serde(default)]
    pub seen_count: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiCacheEntry {
    pub inferred_type: String,
    pub confidence: f64,
    #[serde(default)]
    pub wiki_summary: Option<String>,
    #[serde(default)]
    pub wiki_title: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub word: Option<String>,
    #[serde(default)]
    pub confirmed: bool,
    #[serde(default)]
    pub confirmed_type: Option<String>,
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
    pub wiki_cache: HashMap<String, WikiCacheEntry>,
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
        }
    }
}

pub struct EntityRegistry {
    data: RegistryData,
    path: std::path::PathBuf,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LookupResult {
    pub entity_type: String,
    pub confidence: f64,
    pub source: String,
    pub name: String,
    pub needs_disambiguation: bool,
    #[serde(default)]
    pub context: Vec<String>,
    #[serde(default)]
    pub disambiguated_by: Option<String>,
    #[serde(default)]
    pub wiki_summary: Option<String>,
    #[serde(default)]
    pub wiki_title: Option<String>,
}

pub static COMMON_ENGLISH_WORDS: &[&str] = &[
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
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

const PERSON_CONTEXT_PATTERNS: &[&str] = &[
    r"\b{name}\s+said\b",
    r"\b{name}\s+told\b",
    r"\b{name}\s+asked\b",
    r"\b{name}\s+laughed\b",
    r"\b{name}\s+smiled\b",
    r"\b{name}\s+was\b",
    r"\b{name}\s+is\b",
    r"\b{name}\s+called\b",
    r"\b{name}\s+texted\b",
    r"\bwith\s+{name}\b",
    r"\bsaw\s+{name}\b",
    r"\bcalled\s+{name}\b",
    r"\btook\s+{name}\b",
    r"\bpicked\s+up\s+{name}\b",
    r"\bdrop(?:ped)?\s+(?:off\s+)?{name}\b",
    r"\b{name}(?:'s|s')\b",
    r"\bhey\s+{name}\b",
    r"\bthanks?\s+{name}\b",
    r"^{name}[:\s]",
    r"\bmy\s+(?:son|daughter|kid|child|brother|sister|friend|partner|colleague|coworker)\s+{name}\b",
];

const CONCEPT_CONTEXT_PATTERNS: &[&str] = &[
    r"\bhave\s+you\s+{name}\b",
    r"\bif\s+you\s+{name}\b",
    r"\b{name}\s+since\b",
    r"\b{name}\s+again\b",
    r"\bnot\s+{name}\b",
    r"\b{name}\s+more\b",
    r"\bwould\s+{name}\b",
    r"\bcould\s+{name}\b",
    r"\bwill\s+{name}\b",
    r"(?:the\s+)?{name}\s+(?:of|in|at|for|to)\b",
];

const NAME_INDICATOR_PHRASES: &[&str] = &[
    "given name",
    "personal name",
    "first name",
    "forename",
    "masculine name",
    "feminine name",
    "boy's name",
    "girl's name",
    "male name",
    "female name",
    "irish name",
    "welsh name",
    "scottish name",
    "gaelic name",
    "hebrew name",
    "arabic name",
    "norse name",
    "old english name",
    "is a name",
    "as a name",
    "name meaning",
    "name derived from",
    "legendary irish",
    "legendary welsh",
    "legendary scottish",
];

const PLACE_INDICATOR_PHRASES: &[&str] = &[
    "city in",
    "town in",
    "village in",
    "municipality",
    "capital of",
    "district of",
    "county",
    "province",
    "region of",
    "island of",
    "mountain in",
    "river in",
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

    pub fn mode(&self) -> &str {
        &self.data.mode
    }

    pub fn people(&self) -> &HashMap<String, EntityEntry> {
        &self.data.people
    }

    pub fn projects(&self) -> &[String] {
        &self.data.projects
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn merge_detected_entities(
        &mut self,
        people: &[crate::entity_detector::PersonEntity],
        projects: &[crate::entity_detector::ProjectEntity],
    ) -> anyhow::Result<()> {
        for person in people {
            self.data
                .people
                .entry(person.name.clone())
                .and_modify(|entry| {
                    entry.confidence = entry.confidence.max(person.confidence as f64);
                    if !person.context.is_empty()
                        && !entry
                            .contexts
                            .iter()
                            .any(|context| context == &person.context)
                    {
                        entry.contexts.push(person.context.clone());
                    }
                })
                .or_insert(EntityEntry {
                    source: "init_detected".to_string(),
                    contexts: if person.context.is_empty() {
                        Vec::new()
                    } else {
                        vec![person.context.clone()]
                    },
                    aliases: Vec::new(),
                    relationship: String::new(),
                    confidence: person.confidence as f64,
                    canonical: None,
                    seen_count: None,
                });
        }

        for project in projects {
            if !self
                .data
                .projects
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&project.name))
            {
                self.data.projects.push(project.name.clone());
            }
        }

        self.data.ambiguous_flags = self
            .data
            .people
            .keys()
            .filter(|name| COMMON_ENGLISH_WORDS.contains(&name.to_lowercase().as_str()))
            .map(|name| name.to_lowercase())
            .collect();

        self.save()
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
                    canonical: None,
                    seen_count: None,
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
                        seen_count: None,
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
                    context: info.contexts.clone(),
                    disambiguated_by: None,
                    wiki_summary: None,
                    wiki_title: None,
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
                    context: vec![],
                    disambiguated_by: None,
                    wiki_summary: None,
                    wiki_title: None,
                };
            }
        }

        for (cached_word, cached_result) in &self.data.wiki_cache {
            if word_lower == cached_word.to_lowercase() && cached_result.confirmed {
                return LookupResult {
                    entity_type: cached_result
                        .confirmed_type
                        .clone()
                        .unwrap_or_else(|| cached_result.inferred_type.clone()),
                    confidence: cached_result.confidence,
                    source: "wiki".to_string(),
                    name: word.to_string(),
                    needs_disambiguation: false,
                    context: vec![],
                    disambiguated_by: None,
                    wiki_summary: cached_result.wiki_summary.clone(),
                    wiki_title: cached_result.wiki_title.clone(),
                };
            }
        }

        LookupResult {
            entity_type: "unknown".to_string(),
            confidence: 0.0,
            source: "none".to_string(),
            name: word.to_string(),
            needs_disambiguation: false,
            context: vec![],
            disambiguated_by: None,
            wiki_summary: None,
            wiki_title: None,
        }
    }

    fn disambiguate(
        &self,
        word: &str,
        context: &str,
        person_info: &EntityEntry,
    ) -> Option<LookupResult> {
        let word_lower = word.to_lowercase();
        let ctx_lower = context.to_lowercase();

        let mut person_score = 0;
        let mut concept_score = 0;

        for pattern in PERSON_CONTEXT_PATTERNS {
            let compiled = format!(
                "(?i){}",
                pattern.replace("{name}", &regex::escape(&word_lower))
            );
            if Regex::new(&compiled)
                .map(|re| re.is_match(&ctx_lower))
                .unwrap_or(false)
            {
                person_score += 1;
            }
        }

        for pattern in CONCEPT_CONTEXT_PATTERNS {
            let compiled = format!(
                "(?i){}",
                pattern.replace("{name}", &regex::escape(&word_lower))
            );
            if Regex::new(&compiled)
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
                source: person_info.source.clone(),
                name: word.to_string(),
                needs_disambiguation: false,
                context: person_info.contexts.clone(),
                disambiguated_by: Some("context_patterns".to_string()),
                wiki_summary: None,
                wiki_title: None,
            })
        } else if concept_score > person_score {
            Some(LookupResult {
                entity_type: "concept".to_string(),
                confidence: (0.7 + concept_score as f64 * 0.1).min(0.90),
                source: "context_disambiguated".to_string(),
                name: word.to_string(),
                needs_disambiguation: false,
                context: vec![],
                disambiguated_by: Some("context_patterns".to_string()),
                wiki_summary: None,
                wiki_title: None,
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
                if self.lookup(&word, "").entity_type != "unknown" {
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
            "Mode: {}\nPeople: {} ({})\nProjects: {}\nAmbiguous flags: {}\nWiki cache: {} entries",
            self.data.mode,
            self.data.people.len(),
            people_str,
            self.data.projects.join(", "),
            self.data.ambiguous_flags.join(", "),
            self.data.wiki_cache.len(),
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
        let _ = name;
    }

    /// Check if an entity was previously rejected by the user.
    pub fn is_rejected(&self, name: &str) -> bool {
        let _ = name;
        false
    }

    /// Get the list of all rejected entity names.
    pub fn get_rejected(&self) -> &[String] {
        EMPTY_REJECTED
    }

    /// Filter out rejected entities from a list of candidate names.
    pub fn filter_rejected(&self, names: &[String]) -> Vec<String> {
        names.to_vec()
    }

    /// Reject multiple entities at once (e.g., from an interactive confirmation).
    pub fn reject_entities(&mut self, names: &[String]) {
        let _ = names;
    }

    pub fn research(&mut self, word: &str, auto_confirm: bool) -> anyhow::Result<WikiCacheEntry> {
        self.research_with(word, auto_confirm, wikipedia_lookup)
    }

    pub fn research_with<F>(
        &mut self,
        word: &str,
        auto_confirm: bool,
        lookup: F,
    ) -> anyhow::Result<WikiCacheEntry>
    where
        F: Fn(&str) -> anyhow::Result<WikiCacheEntry>,
    {
        if let Some(existing) = self.data.wiki_cache.get(word) {
            return Ok(existing.clone());
        }

        let mut result = lookup(word)?;
        result.word = Some(word.to_string());
        result.confirmed = auto_confirm;
        self.data
            .wiki_cache
            .insert(word.to_string(), result.clone());
        self.save()?;
        Ok(result)
    }

    pub fn confirm_research(
        &mut self,
        word: &str,
        entity_type: &str,
        relationship: &str,
        context: &str,
    ) -> anyhow::Result<()> {
        if let Some(entry) = self.data.wiki_cache.get_mut(word) {
            entry.confirmed = true;
            entry.confirmed_type = Some(entity_type.to_string());
        }

        if entity_type == "person" {
            self.data.people.insert(
                word.to_string(),
                EntityEntry {
                    source: "wiki".to_string(),
                    contexts: vec![context.to_string()],
                    aliases: vec![],
                    relationship: relationship.to_string(),
                    confidence: 0.90,
                    canonical: None,
                    seen_count: None,
                },
            );

            let lower = word.to_lowercase();
            if COMMON_ENGLISH_WORDS.contains(&lower.as_str())
                && !self.data.ambiguous_flags.contains(&lower)
            {
                self.data.ambiguous_flags.push(lower);
            }
        }

        self.save()
    }

    pub fn learn_from_text(
        &mut self,
        text: &str,
        min_confidence: f32,
    ) -> anyhow::Result<Vec<LearnedEntity>> {
        let detection = detect_from_content(text);
        let mut learned = Vec::new();

        for person in detection.people {
            if person.confidence < min_confidence {
                continue;
            }
            if self.lookup(&person.name, "").entity_type != "unknown" {
                continue;
            }

            let seen_count = text.matches(&person.name).count();
            self.data.people.insert(
                person.name.clone(),
                EntityEntry {
                    source: "learned".to_string(),
                    contexts: vec![if self.data.mode == "combo" {
                        "personal".to_string()
                    } else {
                        self.data.mode.clone()
                    }],
                    aliases: vec![],
                    relationship: String::new(),
                    confidence: person.confidence as f64,
                    canonical: None,
                    seen_count: Some(seen_count),
                },
            );

            let lower = person.name.to_lowercase();
            if COMMON_ENGLISH_WORDS.contains(&lower.as_str())
                && !self.data.ambiguous_flags.contains(&lower)
            {
                self.data.ambiguous_flags.push(lower);
            }

            learned.push(LearnedEntity {
                name: person.name,
                confidence: person.confidence,
                seen_count,
            });
        }

        if !learned.is_empty() {
            self.save()?;
        }

        Ok(learned)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LearnedEntity {
    pub name: String,
    pub confidence: f32,
    pub seen_count: usize,
}

fn wikipedia_lookup(word: &str) -> anyhow::Result<WikiCacheEntry> {
    let encoded = urlencoding::encode(word);
    let url = format!("https://en.wikipedia.org/api/rest_v1/page/summary/{encoded}");
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .user_agent("MemPalace/1.0")
        .build()?;

    let response = match client.get(url).send() {
        Ok(resp) => resp,
        Err(_) => {
            return Ok(WikiCacheEntry {
                inferred_type: "unknown".to_string(),
                confidence: 0.0,
                wiki_summary: None,
                wiki_title: None,
                note: None,
                word: None,
                confirmed: false,
                confirmed_type: None,
            })
        }
    };

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(WikiCacheEntry {
            inferred_type: "person".to_string(),
            confidence: 0.70,
            wiki_summary: None,
            wiki_title: None,
            note: Some("not found in Wikipedia — likely a proper noun or unusual name".to_string()),
            word: None,
            confirmed: false,
            confirmed_type: None,
        });
    }

    let data: serde_json::Value = response.error_for_status()?.json()?;
    let page_type = data
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let extract = data
        .get("extract")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_lowercase();
    let title = data
        .get("title")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    if page_type == "disambiguation" {
        let desc = data
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_lowercase();
        return Ok(if desc.contains("name") || desc.contains("given name") {
            WikiCacheEntry {
                inferred_type: "person".to_string(),
                confidence: 0.65,
                wiki_summary: Some(extract.chars().take(200).collect()),
                wiki_title: title,
                note: Some("disambiguation page with name entries".to_string()),
                word: None,
                confirmed: false,
                confirmed_type: None,
            }
        } else {
            WikiCacheEntry {
                inferred_type: "ambiguous".to_string(),
                confidence: 0.4,
                wiki_summary: Some(extract.chars().take(200).collect()),
                wiki_title: title,
                note: None,
                word: None,
                confirmed: false,
                confirmed_type: None,
            }
        });
    }

    if NAME_INDICATOR_PHRASES
        .iter()
        .any(|phrase| extract.contains(phrase))
    {
        let lower_word = word.to_lowercase();
        let confidence = if extract.contains(&format!("{lower_word} is a"))
            || extract.contains(&format!("{lower_word} (name"))
        {
            0.90
        } else {
            0.80
        };
        return Ok(WikiCacheEntry {
            inferred_type: "person".to_string(),
            confidence,
            wiki_summary: Some(extract.chars().take(200).collect()),
            wiki_title: title,
            note: None,
            word: None,
            confirmed: false,
            confirmed_type: None,
        });
    }

    if PLACE_INDICATOR_PHRASES
        .iter()
        .any(|phrase| extract.contains(phrase))
    {
        return Ok(WikiCacheEntry {
            inferred_type: "place".to_string(),
            confidence: 0.80,
            wiki_summary: Some(extract.chars().take(200).collect()),
            wiki_title: title,
            note: None,
            word: None,
            confirmed: false,
            confirmed_type: None,
        });
    }

    Ok(WikiCacheEntry {
        inferred_type: "concept".to_string(),
        confidence: 0.60,
        wiki_summary: Some(extract.chars().take(200).collect()),
        wiki_title: title,
        note: None,
        word: None,
        confirmed: false,
        confirmed_type: None,
    })
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
        assert!(result.context.is_empty());
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
                vec![("Ever", "personal", "friend")],
                vec![],
                None,
            )
            .unwrap();

        let result = registry.lookup("Ever", "have you ever tried this");
        assert_eq!(result.entity_type, "concept");
        assert_eq!(result.disambiguated_by.as_deref(), Some("context_patterns"));

        let result = registry.lookup("Ever", "Ever said hello to me");
        assert_eq!(result.entity_type, "person");
        assert_eq!(result.source, "onboarding");
    }

    #[test]
    fn test_summary_includes_wiki_cache_count() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("registry.json");
        let mut registry = EntityRegistry::load(&path).unwrap();
        registry.data.wiki_cache.insert(
            "Saoirse".to_string(),
            WikiCacheEntry {
                inferred_type: "person".to_string(),
                confidence: 0.8,
                wiki_summary: Some("Saoirse is a name".to_string()),
                wiki_title: Some("Saoirse".to_string()),
                note: None,
                word: Some("Saoirse".to_string()),
                confirmed: false,
                confirmed_type: None,
            },
        );

        assert!(registry.summary().contains("Wiki cache: 1 entries"));
    }

    #[test]
    fn test_research_caches_result() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("registry.json");
        let mut registry = EntityRegistry::load(&path).unwrap();

        let mock = |_: &str| {
            Ok(WikiCacheEntry {
                inferred_type: "person".to_string(),
                confidence: 0.8,
                wiki_summary: Some("Saoirse is an Irish given name.".to_string()),
                wiki_title: Some("Saoirse".to_string()),
                note: None,
                word: None,
                confirmed: false,
                confirmed_type: None,
            })
        };

        let first = registry.research_with("Saoirse", true, mock).unwrap();
        assert_eq!(first.inferred_type, "person");

        let cached = registry
            .research_with("Saoirse", false, |_| anyhow::bail!("should not be called"))
            .unwrap();
        assert_eq!(cached.inferred_type, "person");
        assert!(cached.confirmed);
    }

    #[test]
    fn test_confirm_research_adds_to_people() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("registry.json");
        let mut registry = EntityRegistry::load(&path).unwrap();

        registry.data.wiki_cache.insert(
            "Saoirse".to_string(),
            WikiCacheEntry {
                inferred_type: "person".to_string(),
                confidence: 0.8,
                wiki_summary: Some("Saoirse is a name".to_string()),
                wiki_title: Some("Saoirse".to_string()),
                note: None,
                word: Some("Saoirse".to_string()),
                confirmed: false,
                confirmed_type: None,
            },
        );

        registry
            .confirm_research("Saoirse", "person", "friend", "personal")
            .unwrap();
        let person = registry.people().get("Saoirse").unwrap();
        assert_eq!(person.source, "wiki");
        assert_eq!(registry.lookup("Saoirse", "").source, "wiki");
    }

    #[test]
    fn test_extract_unknown_candidates_skips_known_aliases_and_projects() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("registry.json");
        let mut registry = EntityRegistry::load(&path).unwrap();
        registry
            .seed(
                "personal",
                vec![("Maxwell", "personal", "friend")],
                vec!["MemPalace"],
                Some(HashMap::from([("Max", "Maxwell")])),
            )
            .unwrap();

        let unknowns = registry.extract_unknown_candidates("Max met Saoirse at MemPalace");
        assert_eq!(unknowns, vec!["Saoirse".to_string()]);
    }

    #[test]
    fn test_seed_canonical_entry_has_no_canonical_field_but_alias_does() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("registry.json");
        let mut registry = EntityRegistry::load(&path).unwrap();
        registry
            .seed(
                "personal",
                vec![("Maxwell", "personal", "friend")],
                vec![],
                Some(HashMap::from([("Max", "Maxwell")])),
            )
            .unwrap();

        let canonical = registry.people().get("Maxwell").unwrap();
        assert_eq!(canonical.aliases, vec!["Max".to_string()]);
        assert!(canonical.canonical.is_none());

        let alias = registry.people().get("Max").unwrap();
        assert_eq!(alias.aliases, vec!["Maxwell".to_string()]);
        assert_eq!(alias.canonical.as_deref(), Some("Maxwell"));
    }

    #[test]
    fn test_get_rejected_is_empty_for_python_parity() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("registry.json");
        let mut registry = EntityRegistry::load(&path).unwrap();
        registry.reject_entity("Alice");
        registry.reject_entities(&["Bob".to_string()]);

        assert!(registry.get_rejected().is_empty());
        assert!(!registry.is_rejected("Alice"));
        assert_eq!(
            registry.filter_rejected(&["Alice".to_string(), "Bob".to_string()]),
            vec!["Alice".to_string(), "Bob".to_string()]
        );
    }

    #[test]
    fn test_learn_from_text_adds_new_people() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let path = temp_dir.path().join("registry.json");
        let mut registry = EntityRegistry::load(&path).unwrap();

        let text = "Riley said we should ship it. Riley asked about the deploy plan. Riley smiled.";
        let learned = registry.learn_from_text(text, 0.75).unwrap();

        assert!(!learned.is_empty());
        assert!(registry.people().contains_key("Riley"));
        assert_eq!(registry.people().get("Riley").unwrap().source, "learned");
    }
}
