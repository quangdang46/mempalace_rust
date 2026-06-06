//! Saturation signal detector for multi-agent coordination problems.
//!
//! Detects coordination issues like duplicate work, stale threads,
//! repeated blockers, and low throughput.

use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

/// Configuration for saturation detection thresholds.
#[derive(Debug, Clone)]
pub struct SaturationConfig {
    /// Time window for counting events (ms). Default: 1 hour.
    pub saturation_window_ms: u64,
    /// Stale thread threshold (ms). Default: 30 min.
    pub stale_thread_after_ms: u64,
    /// Minimum new actions per window. Default: 1.
    pub min_new_actions_per_window: usize,
    /// Repeated blocker threshold. Default: 2.
    pub repeated_blocker_threshold: usize,
    /// Duplicate work threshold. Default: 2.
    pub duplicate_work_threshold: usize,
    /// Coordination chatter threshold. Default: 5.
    pub coordination_chatter_threshold: usize,
    /// Low throughput event threshold. Default: 1.
    pub low_throughput_event_threshold: usize,
}

impl Default for SaturationConfig {
    fn default() -> Self {
        Self {
            saturation_window_ms: 3_600_000,       // 1 hour
            stale_thread_after_ms: 1_800_000,      // 30 min
            min_new_actions_per_window: 1,
            repeated_blocker_threshold: 2,
            duplicate_work_threshold: 2,
            coordination_chatter_threshold: 5,
            low_throughput_event_threshold: 1,
        }
    }
}

/// Types of saturation signals that can be detected.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SaturationSignal {
    /// Too few new actions created in the window.
    FewNewActions,
    /// Same blocker appearing multiple times.
    RepeatedBlockers,
    /// Agents claiming the same work.
    DuplicateWork,
    /// Introduction messages without follow-up claims.
    StaleIntroductionsWithoutClaims,
    /// Many messages but few completions.
    HighChatterLowThroughput,
    /// Threads with no recent activity.
    StaleThreads,
}

/// Evidence for a saturation signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalEvidence {
    pub signal: SaturationSignal,
    pub count: usize,
    pub threshold: usize,
    pub details: Vec<String>,
}

/// Recommendation for skill switching based on saturation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSwitchRecommendation {
    pub signal: SaturationSignal,
    pub recommended_skill: String,
    pub confidence: String, // "high", "medium", "low"
}

/// Complete saturation report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaturationReport {
    pub signals: Vec<SignalEvidence>,
    pub saturated: bool,
    pub reasons: Vec<String>,
    pub recommendations: Vec<SkillSwitchRecommendation>,
}

/// An event for saturation analysis.
#[derive(Debug, Clone)]
pub struct CoordinationEvent {
    pub event_type: String,
    pub agent_id: String,
    pub content: String,
    pub timestamp_ms: u64,
    pub thread_id: Option<String>,
}

/// Blocker fingerprint accumulator.
#[derive(Debug, Clone)]
struct BlockerHotspot {
    fingerprint: String,
    display_key: String,
    sample: String,
    count: usize,
}

/// Saturation detector.
pub struct SaturationDetector {
    config: SaturationConfig,
}

impl SaturationDetector {
    /// Create a new detector with the given config.
    pub fn new(config: SaturationConfig) -> Self {
        Self { config }
    }

    /// Create a detector with default config.
    pub fn with_defaults() -> Self {
        Self::new(SaturationConfig::default())
    }

    /// Analyze events and produce a saturation report.
    pub fn analyze(&self, events: &[CoordinationEvent], now_ms: u64) -> SaturationReport {
        let window_start = now_ms.saturating_sub(self.config.saturation_window_ms);
        let recent: Vec<&CoordinationEvent> = events
            .iter()
            .filter(|e| e.timestamp_ms >= window_start)
            .collect();

        let mut signals = Vec::new();

        // 1. FewNewActions
        if let Some(sig) = self.check_few_new_actions(&recent) {
            signals.push(sig);
        }

        // 2. RepeatedBlockers
        if let Some(sig) = self.check_repeated_blockers(&recent) {
            signals.push(sig);
        }

        // 3. DuplicateWork
        if let Some(sig) = self.check_duplicate_work(&recent) {
            signals.push(sig);
        }

        // 4. StaleIntroductionsWithoutClaims
        if let Some(sig) = self.check_stale_introductions(&recent) {
            signals.push(sig);
        }

        // 5. HighChatterLowThroughput
        if let Some(sig) = self.check_high_chatter_low_throughput(&recent) {
            signals.push(sig);
        }

        // 6. StaleThreads
        if let Some(sig) = self.check_stale_threads(&recent, now_ms) {
            signals.push(sig);
        }

        let saturated = !signals.is_empty();
        let reasons: Vec<String> = signals
            .iter()
            .map(|s| format!("{:?}: {} occurrences (threshold: {})", s.signal, s.count, s.threshold))
            .collect();

        let recommendations = signals
            .iter()
            .filter_map(|s| self.recommend_skill(&s.signal))
            .collect();

        SaturationReport {
            signals,
            saturated,
            reasons,
            recommendations,
        }
    }

    fn check_few_new_actions(&self, events: &[&CoordinationEvent]) -> Option<SignalEvidence> {
        let new_actions = events
            .iter()
            .filter(|e| e.event_type == "action_created")
            .count();

        if new_actions < self.config.min_new_actions_per_window {
            Some(SignalEvidence {
                signal: SaturationSignal::FewNewActions,
                count: new_actions,
                threshold: self.config.min_new_actions_per_window,
                details: vec![format!("Only {} new actions in window", new_actions)],
            })
        } else {
            None
        }
    }

    fn check_repeated_blockers(&self, events: &[&CoordinationEvent]) -> Option<SignalEvidence> {
        let mut fingerprints: BTreeMap<String, usize> = BTreeMap::new();

        for event in events {
            if event.event_type == "blocker" || event.content.contains("blocked") {
                let fp = blocker_fingerprint(&event.content);
                *fingerprints.entry(fp).or_insert(0) += 1;
            }
        }

        let repeated: Vec<(String, usize)> = fingerprints
            .into_iter()
            .filter(|(_, count)| *count >= self.config.repeated_blocker_threshold)
            .collect();

        if !repeated.is_empty() {
            let details: Vec<String> = repeated
                .iter()
                .map(|(fp, count)| format!("{}: {} times", fp, count))
                .collect();
            let total: usize = repeated.iter().map(|(_, c)| c).sum();

            Some(SignalEvidence {
                signal: SaturationSignal::RepeatedBlockers,
                count: total,
                threshold: self.config.repeated_blocker_threshold,
                details,
            })
        } else {
            None
        }
    }

    fn check_duplicate_work(&self, events: &[&CoordinationEvent]) -> Option<SignalEvidence> {
        let duplicate_keywords = ["already claimed", "duplicate", "same task", "same action"];
        let mut duplicate_count = 0;
        let mut details = Vec::new();

        for event in events {
            let content_lower = event.content.to_lowercase();
            if duplicate_keywords.iter().any(|kw| content_lower.contains(kw)) {
                duplicate_count += 1;
                details.push(format!("{}: {}", event.agent_id, truncate(&event.content, 80)));
            }
        }

        if duplicate_count >= self.config.duplicate_work_threshold {
            Some(SignalEvidence {
                signal: SaturationSignal::DuplicateWork,
                count: duplicate_count,
                threshold: self.config.duplicate_work_threshold,
                details,
            })
        } else {
            None
        }
    }

    fn check_stale_introductions(&self, events: &[&CoordinationEvent]) -> Option<SignalEvidence> {
        let intro_keywords = ["hello", "intro", "available", "starting", "beginning"];
        let claim_keywords = ["claimed", "working on", "assigned", "taking"];

        let mut intros: BTreeMap<String, usize> = BTreeMap::new();
        let mut claims: BTreeSet<String> = BTreeSet::new();

        for event in events {
            let content_lower = event.content.to_lowercase();
            if intro_keywords.iter().any(|kw| content_lower.contains(kw)) {
                *intros.entry(event.agent_id.clone()).or_insert(0) += 1;
            }
            if claim_keywords.iter().any(|kw| content_lower.contains(kw)) {
                claims.insert(event.agent_id.clone());
            }
        }

        let stale: Vec<(String, usize)> = intros
            .into_iter()
            .filter(|(agent, count)| !claims.contains(agent) && *count >= 2)
            .collect();

        if stale.len() >= self.config.stale_introductions_without_claims_threshold() {
            let details: Vec<String> = stale
                .iter()
                .map(|(agent, count)| format!("{}: {} intros without claims", agent, count))
                .collect();

            Some(SignalEvidence {
                signal: SaturationSignal::StaleIntroductionsWithoutClaims,
                count: stale.len(),
                threshold: 2,
                details,
            })
        } else {
            None
        }
    }

    fn check_high_chatter_low_throughput(
        &self,
        events: &[&CoordinationEvent],
    ) -> Option<SignalEvidence> {
        let chatter = events
            .iter()
            .filter(|e| e.event_type == "signal_sent" || e.event_type == "message")
            .count();

        let throughput = events
            .iter()
            .filter(|e| {
                e.event_type == "action_completed"
                    || e.event_type == "task_completed"
                    || e.event_type == "verification"
            })
            .count();

        if chatter >= self.config.coordination_chatter_threshold
            && throughput <= self.config.low_throughput_event_threshold
        {
            Some(SignalEvidence {
                signal: SaturationSignal::HighChatterLowThroughput,
                count: chatter,
                threshold: self.config.coordination_chatter_threshold,
                details: vec![format!(
                    "Chatter: {}, Throughput: {}",
                    chatter, throughput
                )],
            })
        } else {
            None
        }
    }

    fn check_stale_threads(
        &self,
        events: &[&CoordinationEvent],
        now_ms: u64,
    ) -> Option<SignalEvidence> {
        let mut thread_activity: BTreeMap<String, u64> = BTreeMap::new();

        for event in events {
            if let Some(thread_id) = &event.thread_id {
                let entry = thread_activity
                    .entry(thread_id.clone())
                    .or_insert(0);
                if event.timestamp_ms > *entry {
                    *entry = event.timestamp_ms;
                }
            }
        }

        let stale_threshold = now_ms.saturating_sub(self.config.stale_thread_after_ms);
        let stale_threads: Vec<String> = thread_activity
            .into_iter()
            .filter(|(_, last_activity)| *last_activity < stale_threshold)
            .map(|(id, _)| id)
            .collect();

        if !stale_threads.is_empty() {
            let details: Vec<String> = stale_threads
                .iter()
                .map(|id| format!("Thread {} inactive for >{}ms", id, self.config.stale_thread_after_ms))
                .collect();

            Some(SignalEvidence {
                signal: SaturationSignal::StaleThreads,
                count: stale_threads.len(),
                threshold: 1,
                details,
            })
        } else {
            None
        }
    }

    fn recommend_skill(&self, signal: &SaturationSignal) -> Option<SkillSwitchRecommendation> {
        match signal {
            SaturationSignal::FewNewActions => Some(SkillSwitchRecommendation {
                signal: signal.clone(),
                recommended_skill: "MockCodeFinder".to_string(),
                confidence: "medium".to_string(),
            }),
            SaturationSignal::RepeatedBlockers => Some(SkillSwitchRecommendation {
                signal: signal.clone(),
                recommended_skill: "TestingConformanceHarnesses".to_string(),
                confidence: "high".to_string(),
            }),
            SaturationSignal::DuplicateWork => Some(SkillSwitchRecommendation {
                signal: signal.clone(),
                recommended_skill: "TestingGoldenArtifacts".to_string(),
                confidence: "high".to_string(),
            }),
            SaturationSignal::StaleIntroductionsWithoutClaims => Some(SkillSwitchRecommendation {
                signal: signal.clone(),
                recommended_skill: "NarrowImplementationBead".to_string(),
                confidence: "medium".to_string(),
            }),
            SaturationSignal::HighChatterLowThroughput => Some(SkillSwitchRecommendation {
                signal: signal.clone(),
                recommended_skill: "Profiling".to_string(),
                confidence: "high".to_string(),
            }),
            SaturationSignal::StaleThreads => Some(SkillSwitchRecommendation {
                signal: signal.clone(),
                recommended_skill: "DeadlockFinder".to_string(),
                confidence: "medium".to_string(),
            }),
        }
    }
}

/// Compute a stable FNV-1a 64-bit hash for blocker evidence.
fn blocker_fingerprint(evidence: &str) -> String {
    let normalized = normalize_evidence(evidence);
    let hash = fnv1a_64(&normalized);
    format!("blocker:{:016x}", hash)
}

/// Normalize evidence for fingerprinting: lowercase, collapse paths/UUIDs/numbers.
fn normalize_evidence(evidence: &str) -> String {
    let lower = evidence.to_lowercase();
    let mut result = String::with_capacity(lower.len());

    for word in lower.split_whitespace() {
        if is_path(word) {
            result.push_str("<path> ");
        } else if is_uuid(word) {
            result.push_str("<uuid> ");
        } else if is_number(word) {
            result.push_str("<num> ");
        } else {
            result.push_str(word);
            result.push(' ');
        }
    }

    result.trim().to_string()
}

fn is_path(s: &str) -> bool {
    s.contains('/') || s.contains('\\') || s.ends_with(".rs") || s.ends_with(".ts")
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36 && s.chars().filter(|c| *c == '-').count() == 4
}

fn is_number(s: &str) -> bool {
    s.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// FNV-1a 64-bit hash.
fn fnv1a_64(data: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET;
    for byte in data.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

/// Extension trait for SaturationConfig.
trait SaturationConfigExt {
    fn stale_introductions_without_claims_threshold(&self) -> usize;
}

impl SaturationConfigExt for SaturationConfig {
    fn stale_introductions_without_claims_threshold(&self) -> usize {
        2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(event_type: &str, agent_id: &str, content: &str, ts: u64) -> CoordinationEvent {
        CoordinationEvent {
            event_type: event_type.to_string(),
            agent_id: agent_id.to_string(),
            content: content.to_string(),
            timestamp_ms: ts,
            thread_id: None,
        }
    }

    fn make_thread_event(
        event_type: &str,
        agent_id: &str,
        content: &str,
        ts: u64,
        thread_id: &str,
    ) -> CoordinationEvent {
        CoordinationEvent {
            event_type: event_type.to_string(),
            agent_id: agent_id.to_string(),
            content: content.to_string(),
            timestamp_ms: ts,
            thread_id: Some(thread_id.to_string()),
        }
    }

    #[test]
    fn test_no_saturation() {
        let detector = SaturationDetector::with_defaults();
        let events = vec![
            make_event("action_created", "a1", "New task", 1000),
            make_event("action_completed", "a1", "Done", 2000),
        ];
        let report = detector.analyze(&events, 3000);
        assert!(!report.saturated);
        assert!(report.signals.is_empty());
    }

    #[test]
    fn test_few_new_actions() {
        let detector = SaturationDetector::with_defaults();
        let events: Vec<CoordinationEvent> = vec![];
        let report = detector.analyze(&events, 3_600_000);
        assert!(report.saturated);
        assert!(report
            .signals
            .iter()
            .any(|s| s.signal == SaturationSignal::FewNewActions));
    }

    #[test]
    fn test_repeated_blockers() {
        let detector = SaturationDetector::with_defaults();
        let events = vec![
            make_event("blocker", "a1", "blocked by CI failure", 1000),
            make_event("blocker", "a1", "blocked by CI failure", 2000),
            make_event("blocker", "a1", "blocked by CI failure", 3000),
        ];
        let report = detector.analyze(&events, 4000);
        assert!(report
            .signals
            .iter()
            .any(|s| s.signal == SaturationSignal::RepeatedBlockers));
    }

    #[test]
    fn test_duplicate_work() {
        let detector = SaturationDetector::with_defaults();
        let events = vec![
            make_event("message", "a1", "Already claimed by a2", 1000),
            make_event("message", "a2", "Duplicate work detected", 2000),
            make_event("message", "a3", "Same task already assigned", 3000),
        ];
        let report = detector.analyze(&events, 4000);
        assert!(report
            .signals
            .iter()
            .any(|s| s.signal == SaturationSignal::DuplicateWork));
    }

    #[test]
    fn test_high_chatter_low_throughput() {
        let detector = SaturationDetector::with_defaults();
        let events: Vec<CoordinationEvent> = (0..6)
            .map(|i| make_event("signal_sent", "a1", "chatter", i * 1000))
            .collect();
        let report = detector.analyze(&events, 7000);
        assert!(report
            .signals
            .iter()
            .any(|s| s.signal == SaturationSignal::HighChatterLowThroughput));
    }

    #[test]
    fn test_stale_threads() {
        let detector = SaturationDetector::with_defaults();
        let events = vec![
            make_thread_event("signal_sent", "a1", "hello", 1000, "thread-1"),
            make_thread_event("signal_sent", "a1", "hello", 2000, "thread-1"),
        ];
        // Now is well past stale threshold
        let report = detector.analyze(&events, 3_600_000);
        assert!(report
            .signals
            .iter()
            .any(|s| s.signal == SaturationSignal::StaleThreads));
    }

    #[test]
    fn test_recommendations() {
        let detector = SaturationDetector::with_defaults();
        let events: Vec<CoordinationEvent> = vec![];
        let report = detector.analyze(&events, 3_600_000);
        assert!(!report.recommendations.is_empty());
    }

    #[test]
    fn test_fnv1a_stable() {
        let a = fnv1a_64("hello world");
        let b = fnv1a_64("hello world");
        assert_eq!(a, b);

        let c = fnv1a_64("hello world!");
        assert_ne!(a, c);
    }

    #[test]
    fn test_blocker_fingerprint_stable() {
        let fp1 = blocker_fingerprint("blocked by CI failure in src/main.rs");
        let fp2 = blocker_fingerprint("blocked by CI failure in src/main.rs");
        assert_eq!(fp1, fp2);

        let fp3 = blocker_fingerprint("blocked by different issue");
        assert_ne!(fp1, fp3);
    }
}
