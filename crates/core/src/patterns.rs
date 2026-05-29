use crate::types::{CompressedObservation, Pattern};
use anyhow::Result;
use chrono::Utc;
use std::collections::{HashMap, HashSet};

pub struct PatternStore {
    patterns: Vec<Pattern>,
}

impl PatternStore {
    pub fn new() -> Self {
        Self {
            patterns: Vec::new(),
        }
    }

    pub fn extract_co_change_patterns(
        &self,
        observations: &[CompressedObservation],
        min_frequency: usize,
    ) -> Result<Vec<Pattern>> {
        let mut file_pairs: HashMap<(String, String), HashSet<String>> = HashMap::new();

        for obs in observations {
            let files: Vec<&String> = obs.files.iter().collect();
            for i in 0..files.len() {
                for j in (i + 1)..files.len() {
                    let (a, b) = if files[i] < files[j] {
                        (files[i].clone(), files[j].clone())
                    } else {
                        (files[j].clone(), files[i].clone())
                    };
                    file_pairs
                        .entry((a, b))
                        .or_insert_with(HashSet::new)
                        .insert(obs.session_id.clone());
                }
            }
        }

        let patterns: Vec<Pattern> = file_pairs
            .into_iter()
            .filter(|(_, sessions)| sessions.len() >= min_frequency)
            .map(|((a, b), sessions)| {
                let now = Utc::now();
                Pattern {
                    id: format!("pat-co-{}-{}", a.replace('/', "_"), b.replace('/', "_")),
                    pattern_type: "co_change".to_string(),
                    description: format!("Files {} and {} are frequently modified together", a, b),
                    files: vec![a, b],
                    frequency: sessions.len(),
                    sessions: sessions.into_iter().collect(),
                    created_at: now,
                    updated_at: now,
                }
            })
            .collect();

        Ok(patterns)
    }

    pub fn extract_error_patterns(
        &self,
        observations: &[CompressedObservation],
        min_frequency: usize,
    ) -> Result<Vec<Pattern>> {
        let mut error_counts: HashMap<String, Vec<String>> = HashMap::new();

        for obs in observations {
            if obs.observation_type == crate::types::ObservationType::Error {
                error_counts
                    .entry(obs.title.clone())
                    .or_default()
                    .push(obs.session_id.clone());
            }
        }

        let patterns: Vec<Pattern> = error_counts
            .into_iter()
            .filter(|(_, sessions)| sessions.len() >= min_frequency)
            .map(|(title, sessions)| {
                let files: HashSet<String> = observations
                    .iter()
                    .filter(|o| o.title == title)
                    .flat_map(|o| o.files.iter().cloned())
                    .collect();
                let now = Utc::now();
                Pattern {
                    id: format!("pat-err-{}", title.replace(|c: char| !c.is_alphanumeric(), "_")),
                    pattern_type: "error_repeat".to_string(),
                    description: format!("Error '{}' occurred {} times", title, sessions.len()),
                    files: files.into_iter().collect(),
                    frequency: sessions.len(),
                    sessions,
                    created_at: now,
                    updated_at: now,
                }
            })
            .collect();

        Ok(patterns)
    }

    pub fn get_patterns(&self) -> &[Pattern] {
        &self.patterns
    }

    pub fn add_pattern(&mut self, pattern: Pattern) {
        self.patterns.push(pattern);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_obs(id: &str, files: Vec<&str>, obs_type: crate::types::ObservationType) -> CompressedObservation {
        CompressedObservation {
            id: id.to_string(),
            session_id: format!("s-{}", id),
            timestamp: Utc::now(),
            observation_type: obs_type,
            title: format!("Title {}", id),
            subtitle: None,
            facts: vec![],
            narrative: "test".to_string(),
            concepts: vec![],
            files: files.into_iter().map(String::from).collect(),
            importance: 5,
            confidence: 0.8,
            image_ref: None,
            image_description: None,
            modality: "text".to_string(),
            agent_id: None,
        }
    }

    #[test]
    fn test_co_change_patterns_finds_frequent_pairs() {
        let store = PatternStore::new();
        let obs = vec![
            test_obs("1", vec!["src/auth.rs", "src/middleware.rs"], crate::types::ObservationType::FileEdit),
            test_obs("2", vec!["src/auth.rs", "src/middleware.rs"], crate::types::ObservationType::FileEdit),
            test_obs("3", vec!["src/auth.rs", "src/middleware.rs"], crate::types::ObservationType::FileEdit),
            test_obs("4", vec!["src/auth.rs", "src/api.rs"], crate::types::ObservationType::FileEdit),
        ];
        let patterns = store.extract_co_change_patterns(&obs, 3).unwrap();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].pattern_type, "co_change");
        assert_eq!(patterns[0].frequency, 3);
    }

    #[test]
    fn test_co_change_below_threshold() {
        let store = PatternStore::new();
        let obs = vec![
            test_obs("1", vec!["src/a.rs", "src/b.rs"], crate::types::ObservationType::FileEdit),
            test_obs("2", vec!["src/a.rs", "src/b.rs"], crate::types::ObservationType::FileEdit),
        ];
        let patterns = store.extract_co_change_patterns(&obs, 3).unwrap();
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_error_repeat_patterns() {
        let store = PatternStore::new();
        let mut obs1 = test_obs("1", vec!["src/auth.rs"], crate::types::ObservationType::Error);
        obs1.title = "Null pointer exception".to_string();
        let mut obs2 = test_obs("2", vec!["src/auth.rs"], crate::types::ObservationType::Error);
        obs2.title = "Null pointer exception".to_string();
        let mut obs3 = test_obs("3", vec!["src/auth.rs"], crate::types::ObservationType::Error);
        obs3.title = "Null pointer exception".to_string();
        let mut obs4 = test_obs("4", vec!["src/api.rs"], crate::types::ObservationType::Error);
        obs4.title = "Timeout error".to_string();
        let patterns = store.extract_error_patterns(&[obs1, obs2, obs3, obs4], 3).unwrap();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].pattern_type, "error_repeat");
    }

    #[test]
    fn test_error_pattern_below_threshold() {
        let store = PatternStore::new();
        let obs = vec![
            test_obs("1", vec![], crate::types::ObservationType::Error),
            test_obs("2", vec![], crate::types::ObservationType::Error),
        ];
        let patterns = store.extract_error_patterns(&obs, 3).unwrap();
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_add_and_get_patterns() {
        let mut store = PatternStore::new();
        let pattern = Pattern {
            id: "pat-1".into(),
            pattern_type: "manual".into(),
            description: "Manual pattern".into(),
            files: vec!["src/main.rs".into()],
            frequency: 1,
            sessions: vec!["s-1".into()],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        store.add_pattern(pattern);
        assert_eq!(store.get_patterns().len(), 1);
    }

    #[test]
    fn test_co_change_with_many_files() {
        let store = PatternStore::new();
        let obs = vec![
            test_obs("1", vec!["a.rs", "b.rs", "c.rs"], crate::types::ObservationType::FileEdit),
            test_obs("2", vec!["a.rs", "b.rs", "c.rs"], crate::types::ObservationType::FileEdit),
            test_obs("3", vec!["a.rs", "b.rs", "c.rs"], crate::types::ObservationType::FileEdit),
        ];
        let patterns = store.extract_co_change_patterns(&obs, 3).unwrap();
        assert!(patterns.len() >= 3);
    }

    #[test]
    fn test_empty_observations() {
        let store = PatternStore::new();
        let co_patterns = store.extract_co_change_patterns(&[], 1).unwrap();
        assert!(co_patterns.is_empty());
        let err_patterns = store.extract_error_patterns(&[], 1).unwrap();
        assert!(err_patterns.is_empty());
    }

    #[test]
    fn test_pattern_id_generation() {
        let store = PatternStore::new();
        let obs = vec![
            test_obs("1", vec!["src/auth.rs", "src/middleware.rs"], crate::types::ObservationType::FileEdit),
            test_obs("2", vec!["src/auth.rs", "src/middleware.rs"], crate::types::ObservationType::FileEdit),
            test_obs("3", vec!["src/auth.rs", "src/middleware.rs"], crate::types::ObservationType::FileEdit),
        ];
        let patterns = store.extract_co_change_patterns(&obs, 3).unwrap();
        assert!(patterns[0].id.starts_with("pat-co-"));
    }
}
