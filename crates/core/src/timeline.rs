use crate::types::{CompressedObservation, TimelineEntry};
use anyhow::Result;
use chrono::{DateTime, Utc};

pub fn build_timeline(
    observations: &[CompressedObservation],
    anchor: Option<&str>,
    before: Option<usize>,
    after: Option<usize>,
) -> Result<Vec<TimelineEntry>> {
    let mut sorted: Vec<&CompressedObservation> = observations.iter().collect();
    sorted.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

    let entries: Vec<TimelineEntry> = sorted
        .iter()
        .map(|obs| TimelineEntry {
            id: format!("tl-{}", obs.id),
            observation_id: obs.id.clone(),
            session_id: obs.session_id.clone(),
            timestamp: obs.timestamp,
            title: obs.title.clone(),
            narrative: obs.narrative.clone(),
            relative_position: "absolute".to_string(),
        })
        .collect();

    if let Some(anchor_str) = anchor {
        return timeline_with_anchor(&entries, anchor_str, before, after);
    }

    Ok(entries)
}

fn timeline_with_anchor(
    entries: &[TimelineEntry],
    anchor: &str,
    before: Option<usize>,
    after: Option<usize>,
) -> Result<Vec<TimelineEntry>> {
    let anchor_pos = entries.iter().position(|e| {
        let ts_str = e.timestamp.format("%Y-%m-%d").to_string();
        e.title.contains(anchor)
            || e.narrative.contains(anchor)
            || ts_str.contains(anchor)
            || e.observation_id.contains(anchor)
    });

    let Some(idx) = anchor_pos else {
        return Ok(entries.to_vec());
    };

    let start = before.map(|b| idx.saturating_sub(b)).unwrap_or(0);
    let end = after
        .map(|a| (idx + a + 1).min(entries.len()))
        .unwrap_or(entries.len());

    Ok(entries[start..end].to_vec())
}

pub fn search_timeline(entries: &[TimelineEntry], query: &str) -> Vec<TimelineEntry> {
    let q = query.to_lowercase();
    entries
        .iter()
        .filter(|e| {
            e.title.to_lowercase().contains(&q)
                || e.narrative.to_lowercase().contains(&q)
                || e.observation_id.contains(&q)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_obs(id: &str, title: &str, days_ago: i64) -> CompressedObservation {
        CompressedObservation {
            id: id.to_string(),
            session_id: "s-1".to_string(),
            timestamp: Utc::now() - chrono::Duration::days(days_ago),
            observation_type: crate::types::ObservationType::FileEdit,
            title: title.to_string(),
            subtitle: None,
            facts: vec![],
            narrative: format!("Narrative for {}", title),
            concepts: vec![],
            files: vec![],
            importance: 5,
            confidence: 0.8,
            image_ref: None,
            image_description: None,
            modality: "text".to_string(),
            agent_id: None,
        }
    }

    #[test]
    fn test_build_timeline_sorts_chronologically() {
        let obs = vec![
            test_obs("o-3", "Third", 1),
            test_obs("o-1", "First", 3),
            test_obs("o-2", "Second", 2),
        ];
        let timeline = build_timeline(&obs, None, None, None).unwrap();
        assert_eq!(timeline.len(), 3);
        assert!(timeline[0].title == "First");
        assert!(timeline[1].title == "Second");
        assert!(timeline[2].title == "Third");
    }

    #[test]
    fn test_build_timeline_empty() {
        let timeline = build_timeline(&[], None, None, None).unwrap();
        assert!(timeline.is_empty());
    }

    #[test]
    fn test_timeline_with_anchor_by_title() {
        let obs: Vec<CompressedObservation> = (0..5)
            .map(|i| test_obs(&format!("o-{}", i), &format!("Edit {}", i), 5 - i))
            .collect();
        let timeline = build_timeline(&obs, Some("Edit 2"), Some(1), Some(1)).unwrap();
        assert!(timeline.len() <= 3);
        let titles: Vec<&str> = timeline.iter().map(|e| e.title.as_str()).collect();
        assert!(titles.iter().any(|t| t.contains("Edit 2")));
    }

    #[test]
    fn test_timeline_anchor_not_found_returns_all() {
        let obs = vec![test_obs("o-1", "Edit auth", 1)];
        let timeline = build_timeline(&obs, Some("nonexistent"), None, None).unwrap();
        assert_eq!(timeline.len(), 1);
    }

    #[test]
    fn test_search_timeline() {
        let entries = vec![
            TimelineEntry {
                id: "tl-1".into(),
                observation_id: "o-1".into(),
                session_id: "s-1".into(),
                timestamp: Utc::now(),
                title: "Implement JWT auth".into(),
                narrative: "Added JWT middleware".into(),
                relative_position: "absolute".into(),
            },
            TimelineEntry {
                id: "tl-2".into(),
                observation_id: "o-2".into(),
                session_id: "s-1".into(),
                timestamp: Utc::now(),
                title: "Fix CSS layout".into(),
                narrative: "Fixed sidebar spacing".into(),
                relative_position: "absolute".into(),
            },
        ];
        let results = search_timeline(&entries, "JWT");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Implement JWT auth");
    }

    #[test]
    fn test_search_timeline_case_insensitive() {
        let entries = vec![TimelineEntry {
            id: "tl-1".into(),
            observation_id: "o-1".into(),
            session_id: "s-1".into(),
            timestamp: Utc::now(),
            title: "Implement JWT auth".into(),
            narrative: "Added JWT middleware".into(),
            relative_position: "absolute".into(),
        }];
        let results = search_timeline(&entries, "jwt");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_timeline_before_offset() {
        let obs: Vec<CompressedObservation> = (0..10)
            .map(|i| test_obs(&format!("o-{}", i), &format!("Edit {}", i), 10 - i))
            .collect();
        let timeline = build_timeline(&obs, Some("Edit 5"), Some(2), None).unwrap();
        assert!(timeline.iter().any(|e| e.title.contains("Edit 5")));
    }

    #[test]
    fn test_timeline_after_offset() {
        let obs: Vec<CompressedObservation> = (0..10)
            .map(|i| test_obs(&format!("o-{}", i), &format!("Edit {}", i), 10 - i))
            .collect();
        let timeline = build_timeline(&obs, Some("Edit 5"), None, Some(2)).unwrap();
        assert!(timeline.iter().any(|e| e.title.contains("Edit 5")));
    }
}
