//! Verification — port of upstream `verify.ts`.
//!
//! Verifies a memory or observation by tracing its citation chain back to source.

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Verification result for a memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub id: String,
    pub verified: bool,
    pub chain: Vec<VerificationStep>,
    pub confidence: f64,
    pub issues: Vec<String>,
}

/// A step in the verification chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationStep {
    pub id: String,
    pub source_type: String,
    pub description: String,
    pub verified: bool,
}

/// Verify a memory by tracing its citation chain.
pub fn verify_memory(
    memory_id: &str,
    memories: &[crate::types::Memory],
    observations: &[crate::types::CompressedObservation],
) -> Result<VerificationResult> {
    let memory = memories.iter().find(|m| m.id == memory_id)
        .ok_or_else(|| anyhow::anyhow!("Memory {} not found", memory_id))?;

    let mut chain = Vec::new();
    let mut issues = Vec::new();
    let mut all_verified = true;

    // Step 1: Check memory has source observations
    if memory.source_observation_ids.is_empty() {
        issues.push("No source observations linked".to_string());
        all_verified = false;
    } else {
        for obs_id in &memory.source_observation_ids {
            let obs = observations.iter().find(|o| o.id == *obs_id);
            if let Some(o) = obs {
                chain.push(VerificationStep {
                    id: obs_id.clone(),
                    source_type: "observation".to_string(),
                    description: format!("Observation: {}", o.title),
                    verified: true,
                });
            } else {
                chain.push(VerificationStep {
                    id: obs_id.clone(),
                    source_type: "observation".to_string(),
                    description: format!("Observation {} not found", obs_id),
                    verified: false,
                });
                all_verified = false;
                issues.push(format!("Source observation {} not found", obs_id));
            }
        }
    }

    // Step 2: Check memory relations
    for related_id in &memory.related_ids {
        let related = memories.iter().find(|m| m.id == *related_id);
        if let Some(r) = related {
            chain.push(VerificationStep {
                id: related_id.clone(),
                source_type: "memory".to_string(),
                description: format!("Related memory: {}", r.title),
                verified: true,
            });
        } else {
            chain.push(VerificationStep {
                id: related_id.clone(),
                source_type: "memory".to_string(),
                description: format!("Related memory {} not found", related_id),
                verified: false,
            });
            all_verified = false;
            issues.push(format!("Related memory {} not found", related_id));
        }
    }

    // Step 3: Check supersedes chain
    for sup_id in &memory.supersedes {
        let sup = memories.iter().find(|m| m.id == *sup_id);
        if let Some(s) = sup {
            chain.push(VerificationStep {
                id: sup_id.clone(),
                source_type: "superseded".to_string(),
                description: format!("Superseded: {}", s.title),
                verified: true,
            });
        } else {
            issues.push(format!("Superseded memory {} not found", sup_id));
        }
    }

    // Calculate confidence based on chain
    let verified_steps = chain.iter().filter(|s| s.verified).count();
    let total_steps = chain.len().max(1);
    let base_confidence = verified_steps as f64 / total_steps as f64;

    // Factor in memory's own strength
    let confidence = base_confidence * memory.strength;

    Ok(VerificationResult {
        id: memory_id.to_string(),
        verified: all_verified && issues.is_empty(),
        chain,
        confidence,
        issues,
    })
}

/// Verify an observation by checking its session exists.
pub fn verify_observation(
    obs_id: &str,
    observations: &[crate::types::CompressedObservation],
    session_ids: &[String],
) -> Result<VerificationResult> {
    let obs = observations.iter().find(|o| o.id == obs_id)
        .ok_or_else(|| anyhow::anyhow!("Observation {} not found", obs_id))?;

    let session_exists = session_ids.contains(&obs.session_id);
    let verified = session_exists;

    let mut issues = Vec::new();
    if !session_exists {
        issues.push(format!("Session {} not found", obs.session_id));
    }

    Ok(VerificationResult {
        id: obs_id.to_string(),
        verified,
        chain: vec![VerificationStep {
            id: obs.session_id.clone(),
            source_type: "session".to_string(),
            description: format!("Session {}", obs.session_id),
            verified: session_exists,
        }],
        confidence: if verified { obs.confidence } else { 0.0 },
        issues,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CompressedObservation, Memory, MemoryType, ObservationType};

    fn test_memory(id: &str, source_obs: Vec<&str>, related: Vec<&str>, strength: f64) -> Memory {
        Memory {
            id: id.into(), created_at: chrono::Utc::now(), updated_at: chrono::Utc::now(),
            memory_type: MemoryType::Semantic, title: format!("Memory {}", id),
            content: "test".into(), concepts: vec![], files: vec![], session_ids: vec![],
            strength, version: 1, parent_id: None,
            supersedes: vec![], related_ids: related.into_iter().map(String::from).collect(),
            source_observation_ids: source_obs.into_iter().map(String::from).collect(),
            is_latest: true, forget_after: None, image_ref: None, agent_id: None,
            project: "test".into(),
        }
    }

    fn test_obs(id: &str, session_id: &str) -> CompressedObservation {
        CompressedObservation {
            id: id.into(), session_id: session_id.into(),
            timestamp: chrono::Utc::now(), observation_type: ObservationType::FileEdit,
            title: format!("Obs {}", id), subtitle: None, facts: vec![],
            narrative: "test".into(), concepts: vec![], files: vec![],
            importance: 5, confidence: 0.8, image_ref: None, image_description: None,
            modality: "text".into(), agent_id: None,
        }
    }

    #[test]
    fn test_verify_memory_success() {
        let memories = vec![test_memory("m-1", vec!["o-1"], vec![], 0.9)];
        let observations = vec![test_obs("o-1", "s-1")];
        let result = verify_memory("m-1", &memories, &observations).unwrap();
        assert!(result.verified);
        assert!(result.issues.is_empty());
        assert!(result.confidence > 0.0);
    }

    #[test]
    fn test_verify_memory_missing_observation() {
        let memories = vec![test_memory("m-1", vec!["o-missing"], vec![], 0.9)];
        let observations: Vec<CompressedObservation> = vec![];
        let result = verify_memory("m-1", &memories, &observations).unwrap();
        assert!(!result.verified);
        assert!(!result.issues.is_empty());
    }

    #[test]
    fn test_verify_memory_with_relations() {
        let memories = vec![
            test_memory("m-1", vec!["o-1"], vec!["m-2"], 0.9),
            test_memory("m-2", vec!["o-2"], vec![], 0.8),
        ];
        let observations = vec![test_obs("o-1", "s-1"), test_obs("o-2", "s-1")];
        let result = verify_memory("m-1", &memories, &observations).unwrap();
        assert!(result.verified);
        assert_eq!(result.chain.len(), 2);
    }

    #[test]
    fn test_verify_observation_success() {
        let observations = vec![test_obs("o-1", "s-1")];
        let sessions = vec!["s-1".to_string()];
        let result = verify_observation("o-1", &observations, &sessions).unwrap();
        assert!(result.verified);
    }

    #[test]
    fn test_verify_observation_missing_session() {
        let observations = vec![test_obs("o-1", "s-1")];
        let sessions: Vec<String> = vec![];
        let result = verify_observation("o-1", &observations, &sessions).unwrap();
        assert!(!result.verified);
        assert!(!result.issues.is_empty());
    }

    #[test]
    fn test_verify_memory_not_found() {
        let memories: Vec<Memory> = vec![];
        let observations: Vec<CompressedObservation> = vec![];
        let result = verify_memory("nonexistent", &memories, &observations);
        assert!(result.is_err());
    }
}
