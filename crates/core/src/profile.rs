use crate::types::{CompressedObservation, FrequencyEntry, ProjectProfile};
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const CACHE_TTL_SECONDS: i64 = 3600;

struct ProfileCache {
    profile: Option<ProjectProfile>,
    cached_at: Option<DateTime<Utc>>,
}

pub struct ProfileStore {
    project: String,
    cache: Arc<Mutex<ProfileCache>>,
}

impl ProfileStore {
    pub fn new(project: &str) -> Self {
        Self {
            project: project.to_string(),
            cache: Arc::new(Mutex::new(ProfileCache {
                profile: None,
                cached_at: None,
            })),
        }
    }

    pub fn compute_profile(
        &self,
        observations: &[CompressedObservation],
        session_count: usize,
    ) -> Result<ProjectProfile> {
        let now = Utc::now();

        let mut concept_freq: HashMap<String, usize> = HashMap::new();
        let mut file_freq: HashMap<String, usize> = HashMap::new();
        let mut pattern_freq: HashMap<String, usize> = HashMap::new();
        let mut conventions: Vec<String> = Vec::new();
        let mut common_errors: Vec<String> = Vec::new();

        for obs in observations {
            for concept in &obs.concepts {
                *concept_freq.entry(concept.clone()).or_insert(0) += 1;
            }
            for file in &obs.files {
                *file_freq.entry(file.clone()).or_insert(0) += 1;
            }
            if obs.observation_type == crate::types::ObservationType::Error {
                common_errors.push(obs.title.clone());
            }
        }

        let mut top_concepts: Vec<FrequencyEntry> = concept_freq
            .into_iter()
            .map(|(k, v)| FrequencyEntry { key: k, frequency: v })
            .collect();
        top_concepts.sort_by(|a, b| b.frequency.cmp(&a.frequency));
        top_concepts.truncate(20);

        let mut top_files: Vec<FrequencyEntry> = file_freq
            .into_iter()
            .map(|(k, v)| FrequencyEntry { key: k, frequency: v })
            .collect();
        top_files.sort_by(|a, b| b.frequency.cmp(&a.frequency));
        top_files.truncate(20);

        let mut top_patterns: Vec<FrequencyEntry> = pattern_freq
            .into_iter()
            .map(|(k, v)| FrequencyEntry { key: k, frequency: v })
            .collect();
        top_patterns.sort_by(|a, b| b.frequency.cmp(&a.frequency));
        top_patterns.truncate(10);

        common_errors.sort();
        common_errors.dedup();
        common_errors.truncate(10);

        let recent: Vec<String> = observations
            .iter()
            .rev()
            .take(10)
            .map(|o| o.title.clone())
            .collect();

        let profile = ProjectProfile {
            project: self.project.clone(),
            top_concepts,
            top_files,
            top_patterns,
            conventions,
            common_errors,
            recent_activity: recent,
            session_count,
            total_observations: observations.len(),
            language: None,
            framework: None,
            updated_at: now,
        };

        let mut cache = self.cache.lock().unwrap();
        cache.profile = Some(profile.clone());
        cache.cached_at = Some(now);

        Ok(profile)
    }

    pub fn get_profile(&self) -> Option<ProjectProfile> {
        let cache = self.cache.lock().unwrap();
        if let (Some(profile), Some(cached_at)) = (&cache.profile, cache.cached_at) {
            if Utc::now() - cached_at < Duration::seconds(CACHE_TTL_SECONDS) {
                return Some(profile.clone());
            }
        }
        None
    }

    pub fn invalidate_cache(&self) {
        let mut cache = self.cache.lock().unwrap();
        cache.profile = None;
        cache.cached_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_observation(id: &str, concepts: Vec<&str>, files: Vec<&str>, obs_type: crate::types::ObservationType) -> CompressedObservation {
        CompressedObservation {
            id: id.to_string(),
            session_id: "s-1".to_string(),
            timestamp: Utc::now(),
            observation_type: obs_type,
            title: format!("Title {}", id),
            subtitle: None,
            facts: vec![],
            narrative: "test".to_string(),
            concepts: concepts.into_iter().map(String::from).collect(),
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
    fn test_compute_profile_aggregates_concepts() {
        let store = ProfileStore::new("test-project");
        let obs = vec![
            test_observation("1", vec!["auth", "jwt"], vec!["src/auth.rs"], crate::types::ObservationType::FileEdit),
            test_observation("2", vec!["auth", "api"], vec!["src/api.rs"], crate::types::ObservationType::FileEdit),
            test_observation("3", vec!["jwt"], vec!["src/auth.rs"], crate::types::ObservationType::FileEdit),
        ];
        let profile = store.compute_profile(&obs, 2).unwrap();
        assert_eq!(profile.project, "test-project");
        assert_eq!(profile.session_count, 2);
        assert_eq!(profile.total_observations, 3);
        assert!(!profile.top_concepts.is_empty());
        let auth_entry = profile.top_concepts.iter().find(|e| e.key == "auth").unwrap();
        assert_eq!(auth_entry.frequency, 2);
    }

    #[test]
    fn test_compute_profile_aggregates_files() {
        let store = ProfileStore::new("test-project");
        let obs = vec![
            test_observation("1", vec![], vec!["src/auth.rs", "src/main.rs"], crate::types::ObservationType::FileEdit),
            test_observation("2", vec![], vec!["src/auth.rs"], crate::types::ObservationType::FileEdit),
        ];
        let profile = store.compute_profile(&obs, 1).unwrap();
        let auth_entry = profile.top_files.iter().find(|e| e.key == "src/auth.rs").unwrap();
        assert_eq!(auth_entry.frequency, 2);
    }

    #[test]
    fn test_compute_profile_captures_errors() {
        let store = ProfileStore::new("test-project");
        let mut obs1 = test_observation("1", vec![], vec![], crate::types::ObservationType::Error);
        obs1.title = "Same error".to_string();
        let mut obs2 = test_observation("2", vec![], vec![], crate::types::ObservationType::Error);
        obs2.title = "Same error".to_string();
        let obs3 = test_observation("3", vec![], vec![], crate::types::ObservationType::Other);
        let profile = store.compute_profile(&[obs1, obs2, obs3], 1).unwrap();
        assert_eq!(profile.common_errors.len(), 1);
    }

    #[test]
    fn test_compute_profile_recent_activity() {
        let store = ProfileStore::new("test-project");
        let obs: Vec<CompressedObservation> = (0..15)
            .map(|i| test_observation(&i.to_string(), vec![], vec![], crate::types::ObservationType::Other))
            .collect();
        let profile = store.compute_profile(&obs, 1).unwrap();
        assert!(profile.recent_activity.len() <= 10);
    }

    #[test]
    fn test_cache_ttl() {
        let store = ProfileStore::new("test-project");
        let obs = vec![test_observation("1", vec![], vec![], crate::types::ObservationType::Other)];
        store.compute_profile(&obs, 1).unwrap();
        let cached = store.get_profile();
        assert!(cached.is_some());
    }

    #[test]
    fn test_invalidate_cache() {
        let store = ProfileStore::new("test-project");
        let obs = vec![test_observation("1", vec![], vec![], crate::types::ObservationType::Other)];
        store.compute_profile(&obs, 1).unwrap();
        store.invalidate_cache();
        assert!(store.get_profile().is_none());
    }

    #[test]
    fn test_empty_observations() {
        let store = ProfileStore::new("test-project");
        let profile = store.compute_profile(&[], 0).unwrap();
        assert!(profile.top_concepts.is_empty());
        assert!(profile.top_files.is_empty());
        assert_eq!(profile.total_observations, 0);
    }

    #[test]
    fn test_profile_truncation_limits() {
        let store = ProfileStore::new("test-project");
        let obs: Vec<CompressedObservation> = (0..50)
            .map(|i| {
                test_observation(
                    &i.to_string(),
                    vec![&format!("concept-{}", i)],
                    vec![&format!("file-{}.rs", i)],
                    crate::types::ObservationType::Other,
                )
            })
            .collect();
        let profile = store.compute_profile(&obs, 1).unwrap();
        assert!(profile.top_concepts.len() <= 20);
        assert!(profile.top_files.len() <= 20);
        assert!(profile.top_patterns.len() <= 10);
        assert!(profile.common_errors.len() <= 10);
    }
}
