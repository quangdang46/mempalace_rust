//! LongMemEval-S evaluation harness against `searcher::search_memories`
//! (mp-003, Phase 0).
#![allow(deprecated)]

//! For every question in the dataset we:
//!
//! 1. Wipe a fresh palace under `target/longmemeval_palace/<question_id>/`.
//! 2. Mine each haystack session as a single drawer whose `source_file`
//!    metadata stores the upstream `session_id` — this is what
//!    `SearchResult::from(QueryResult)` will surface as `result.source_file`.
//! 3. Call `mempalace_core::searcher::search_memories(query, ..., 10)` —
//!    explicitly the same API a CLI/MCP user hits, so the number we report
//!    is the *real* Rust port retrieval performance, not a synthetic
//!    embedvec rerank like `crate::runner` runs.
//! 4. Score Recall@5 / Recall@10 / MRR against the upstream
//!    `answer_session_ids`.
//!
//! The hot path is `Send + Sync`-clean and runs serially (one palace
//! at a time) because each question wants an isolated search index.
//! Concurrency is bounded by `--concurrency` so memory stays predictable
//! on smaller boxes.
//!
//! Output:
//! * **NDJSON** — one JSON object per question on stdout (`type=question`)
//!   followed by a single summary footer line (`type=summary`). Designed
//!   so future agents can `diff <(jq -c .)` two runs.
//! * **JSON report** — full structured results dumped to a path the CLI
//!   chooses (`target/longmemeval_results.json` by default).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use mempalace_core::palace_db::PalaceDb;
use mempalace_core::searcher::search_memories;
use serde::Serialize;

use crate::dataset::BenchmarkEntry;

/// One question's per-question result, written verbatim as one NDJSON
/// line and aggregated into the final JSON report.
#[derive(Debug, Clone, Serialize)]
pub struct QuestionRecord {
    /// Tag so consumers can filter `type=question` vs `type=summary`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub question_id: String,
    pub question_type: String,
    pub n_haystack_sessions: usize,
    pub n_answer_sessions: usize,
    pub n_results_returned: usize,
    pub recall_at_5: f64,
    pub recall_at_10: f64,
    pub reciprocal_rank: f64,
    pub correct_first_rank: Option<usize>,
    pub elapsed_ms: u64,
    pub mine_ms: u64,
    pub search_ms: u64,
    /// Top-10 retrieved session IDs in rank order — handy when diffing
    /// two runs to see *why* a number changed.
    pub retrieved_session_ids: Vec<String>,
    /// Whether the question was scored at all (skipped when haystack is
    /// empty after mining or when mining/search errored).
    pub scored: bool,
    /// Populated only when `scored == false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
}

/// Summary footer line written after every per-question line.
#[derive(Debug, Clone, Serialize)]
pub struct SummaryRecord {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub total_questions: usize,
    pub scored: usize,
    pub skipped: usize,
    pub recall_at_5: f64,
    pub recall_at_10: f64,
    pub mrr: f64,
    pub mean_elapsed_ms: f64,
    pub limit_per_search: usize,
    /// Git-friendly fingerprint of the harness — bumping it invalidates
    /// historical numbers, so keep in lockstep with material harness
    /// changes.
    pub harness_version: &'static str,
}

/// Aggregated report dumped to disk as plain JSON.
#[derive(Debug, Clone, Serialize)]
pub struct BenchReport {
    pub summary: SummaryRecord,
    pub records: Vec<QuestionRecord>,
}

/// Bumped whenever the corpus-construction or scoring code changes in
/// a way that should invalidate cached baseline numbers.
pub const HARNESS_VERSION: &str = "mp-003.v1";

/// Hardcoded — the task brief specifies R@5 / R@10 / MRR with limit=10.
pub const SEARCH_LIMIT: usize = 10;

/// Extract the user-side prose from a session as a single string.
/// We keep `assistant` turns out so retrieval matches what
/// `convo_miner::mine_exchange_pairs` files in production.
fn render_session_content(session: &[crate::dataset::Turn]) -> String {
    session
        .iter()
        .filter(|turn| turn.role == "user")
        .map(|turn| turn.content.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Slug a question_id into a path-safe segment. LongMemEval IDs are
/// already file-safe but we strip `/` defensively to keep the palace
/// path layout flat.
fn slug(id: &str) -> String {
    id.replace(['/', '\\'], "_")
}

/// Mine one entry's haystack into a fresh palace.
///
/// Returns the count of drawers actually written (0 if every haystack
/// session was empty after filtering to user turns).
fn mine_entry_into(palace_path: &Path, entry: &BenchmarkEntry) -> Result<usize> {
    // Wipe any leftover from a previous run. We isolate per-question so
    // R@k is computed against just this entry's haystack.
    if palace_path.exists() {
        std::fs::remove_dir_all(palace_path)
            .with_context(|| format!("wiping {}", palace_path.display()))?;
    }
    std::fs::create_dir_all(palace_path)?;

    let mut db = PalaceDb::open(palace_path)
        .with_context(|| format!("opening palace at {}", palace_path.display()))?;

    let mut written = 0usize;
    let qid = slug(&entry.question_id);

    // We pre-allocate the (id, content, metadata-vec) tuples to keep
    // `db.add` happy with its borrowed-slice signature.
    let mut staged: Vec<(String, String, String)> =
        Vec::with_capacity(entry.haystack_sessions.len());

    for (i, session) in entry.haystack_sessions.iter().enumerate() {
        let session_id = entry
            .haystack_session_ids
            .get(i)
            .cloned()
            .unwrap_or_else(|| format!("sess_{i:04}"));

        let content = render_session_content(session);
        if content.is_empty() {
            continue;
        }

        let drawer_id = format!("drawer_lme_{}_{}", qid, slug(&session_id));
        staged.push((drawer_id, content, session_id));
    }

    for (drawer_id, content, session_id) in &staged {
        let meta: Vec<(&str, &str)> = vec![
            ("wing", "longmemeval"),
            ("room", "haystack"),
            ("source_file", session_id.as_str()),
            ("added_by", "longmemeval-bench"),
            ("chunk_index", "0"),
        ];
        db.add(
            &[(drawer_id.as_str(), content.as_str())],
            &[meta.as_slice()],
        )
        .with_context(|| format!("inserting drawer {drawer_id}"))?;
        written += 1;
    }

    db.flush()?;
    Ok(written)
}

/// Compute (recall@5, recall@10, reciprocal_rank, first_correct_rank).
///
/// Recall here is the standard "any hit" definition (Python reference's
/// `recall_at_k`): 1.0 if at least one of the top-K retrieved IDs is in
/// the ground truth, else 0.0. MRR is `1 / rank_of_first_correct`,
/// using 1-based ranks per the LongMemEval paper.
fn score(retrieved: &[String], ground_truth: &HashSet<String>) -> (f64, f64, f64, Option<usize>) {
    if ground_truth.is_empty() || retrieved.is_empty() {
        return (0.0, 0.0, 0.0, None);
    }

    let mut first_hit: Option<usize> = None;
    for (i, id) in retrieved.iter().enumerate() {
        if ground_truth.contains(id) {
            first_hit = Some(i + 1); // 1-based
            break;
        }
    }

    let r5 = retrieved.iter().take(5).any(|id| ground_truth.contains(id));
    let r10 = retrieved
        .iter()
        .take(10)
        .any(|id| ground_truth.contains(id));

    let mrr = first_hit.map(|r| 1.0 / r as f64).unwrap_or(0.0);

    (
        if r5 { 1.0 } else { 0.0 },
        if r10 { 1.0 } else { 0.0 },
        mrr,
        first_hit,
    )
}

/// Run the entire dataset, emitting NDJSON to `ndjson_writer` as we go
/// and accumulating an aggregated [`BenchReport`].
///
/// Errors during a single question's mine/search are caught and recorded
/// as `skipped` rather than aborting the whole run — long benches should
/// always finish.
pub async fn run<W: std::io::Write>(
    entries: &[BenchmarkEntry],
    palace_root: &Path,
    limit: Option<usize>,
    mut ndjson_writer: W,
) -> Result<BenchReport> {
    std::fs::create_dir_all(palace_root)?;

    let total = entries.len();
    let n_to_run = limit.map(|l| l.min(total)).unwrap_or(total);
    let mut records: Vec<QuestionRecord> = Vec::with_capacity(n_to_run);

    let mut acc_r5 = 0f64;
    let mut acc_r10 = 0f64;
    let mut acc_mrr = 0f64;
    let mut acc_elapsed: u128 = 0;
    let mut scored = 0usize;
    let mut skipped = 0usize;

    for (idx, entry) in entries.iter().take(n_to_run).enumerate() {
        let q_started = Instant::now();
        let palace_path = palace_root.join(slug(&entry.question_id));

        let mine_started = Instant::now();
        let mine_outcome = mine_entry_into(&palace_path, entry);
        let mine_ms = mine_started.elapsed().as_millis() as u64;

        let drawers_written = match mine_outcome {
            Ok(0) => {
                let rec = QuestionRecord {
                    kind: "question",
                    question_id: entry.question_id.clone(),
                    question_type: entry.question_type.clone(),
                    n_haystack_sessions: entry.haystack_sessions.len(),
                    n_answer_sessions: entry.answer_session_ids.len(),
                    n_results_returned: 0,
                    recall_at_5: 0.0,
                    recall_at_10: 0.0,
                    reciprocal_rank: 0.0,
                    correct_first_rank: None,
                    elapsed_ms: q_started.elapsed().as_millis() as u64,
                    mine_ms,
                    search_ms: 0,
                    retrieved_session_ids: Vec::new(),
                    scored: false,
                    skip_reason: Some("empty_haystack_after_filter".into()),
                };
                emit_ndjson(&mut ndjson_writer, &rec)?;
                records.push(rec);
                skipped += 1;
                continue;
            }
            Ok(n) => n,
            Err(e) => {
                let rec = QuestionRecord {
                    kind: "question",
                    question_id: entry.question_id.clone(),
                    question_type: entry.question_type.clone(),
                    n_haystack_sessions: entry.haystack_sessions.len(),
                    n_answer_sessions: entry.answer_session_ids.len(),
                    n_results_returned: 0,
                    recall_at_5: 0.0,
                    recall_at_10: 0.0,
                    reciprocal_rank: 0.0,
                    correct_first_rank: None,
                    elapsed_ms: q_started.elapsed().as_millis() as u64,
                    mine_ms,
                    search_ms: 0,
                    retrieved_session_ids: Vec::new(),
                    scored: false,
                    skip_reason: Some(format!("mine_error: {e:#}")),
                };
                emit_ndjson(&mut ndjson_writer, &rec)?;
                records.push(rec);
                skipped += 1;
                continue;
            }
        };
        let _ = drawers_written;

        let search_started = Instant::now();
        // Use the default vector embedder so production search uses hybrid
        // RRF (vector + BM25), not naive Jaccard word-overlap.
        let embed_model = std::env::var("MEMPALACE_EMBED_MODEL")
            .ok()
            .unwrap_or_else(|| "bge-small-en-v15".to_string());
        let response = match search_memories(
            &entry.question,
            &palace_path,
            None,
            None,
            SEARCH_LIMIT,
            Some(&embed_model),
        ) {
            Ok(r) => r,
            Err(e) => {
                let rec = QuestionRecord {
                    kind: "question",
                    question_id: entry.question_id.clone(),
                    question_type: entry.question_type.clone(),
                    n_haystack_sessions: entry.haystack_sessions.len(),
                    n_answer_sessions: entry.answer_session_ids.len(),
                    n_results_returned: 0,
                    recall_at_5: 0.0,
                    recall_at_10: 0.0,
                    reciprocal_rank: 0.0,
                    correct_first_rank: None,
                    elapsed_ms: q_started.elapsed().as_millis() as u64,
                    mine_ms,
                    search_ms: search_started.elapsed().as_millis() as u64,
                    retrieved_session_ids: Vec::new(),
                    scored: false,
                    skip_reason: Some(format!("search_error: {e:#}")),
                };
                emit_ndjson(&mut ndjson_writer, &rec)?;
                records.push(rec);
                skipped += 1;
                continue;
            }
        };
        let search_ms = search_started.elapsed().as_millis() as u64;

        let retrieved_session_ids: Vec<String> = response
            .results
            .iter()
            .map(|r| r.source_file.clone())
            .collect();

        let truth: HashSet<String> = entry.answer_session_ids.iter().cloned().collect();

        let (r5, r10, rr, first_hit) = score(&retrieved_session_ids, &truth);

        acc_r5 += r5;
        acc_r10 += r10;
        acc_mrr += rr;
        let elapsed = q_started.elapsed();
        acc_elapsed += elapsed.as_millis();
        scored += 1;

        let rec = QuestionRecord {
            kind: "question",
            question_id: entry.question_id.clone(),
            question_type: entry.question_type.clone(),
            n_haystack_sessions: entry.haystack_sessions.len(),
            n_answer_sessions: entry.answer_session_ids.len(),
            n_results_returned: retrieved_session_ids.len(),
            recall_at_5: r5,
            recall_at_10: r10,
            reciprocal_rank: rr,
            correct_first_rank: first_hit,
            elapsed_ms: elapsed.as_millis() as u64,
            mine_ms,
            search_ms,
            retrieved_session_ids,
            scored: true,
            skip_reason: None,
        };
        emit_ndjson(&mut ndjson_writer, &rec)?;
        records.push(rec);

        // Log progress every 25 entries so a 500-q run does not look hung.
        if (idx + 1) % 25 == 0 {
            let so_far = (idx + 1) as f64;
            eprintln!(
                "[longmemeval-bench] {}/{} processed (R@5={:.3} R@10={:.3} MRR={:.3})",
                idx + 1,
                n_to_run,
                acc_r5 / so_far.max(1.0),
                acc_r10 / so_far.max(1.0),
                acc_mrr / so_far.max(1.0),
            );
        }
    }

    let denom = scored.max(1) as f64;
    let mean_elapsed_ms = if scored == 0 {
        0.0
    } else {
        acc_elapsed as f64 / scored as f64
    };

    let summary = SummaryRecord {
        kind: "summary",
        total_questions: n_to_run,
        scored,
        skipped,
        recall_at_5: if scored == 0 { 0.0 } else { acc_r5 / denom },
        recall_at_10: if scored == 0 { 0.0 } else { acc_r10 / denom },
        mrr: if scored == 0 { 0.0 } else { acc_mrr / denom },
        mean_elapsed_ms,
        limit_per_search: SEARCH_LIMIT,
        harness_version: HARNESS_VERSION,
    };

    emit_ndjson(&mut ndjson_writer, &summary)?;

    Ok(BenchReport { summary, records })
}

/// One JSON object per call, terminated with `\n` for proper NDJSON.
fn emit_ndjson<W: std::io::Write, T: Serialize>(w: &mut W, value: &T) -> Result<()> {
    let line = serde_json::to_string(value)?;
    w.write_all(line.as_bytes())?;
    w.write_all(b"\n")?;
    w.flush().ok();
    Ok(())
}

/// Convenience: write the aggregated report as pretty-printed JSON.
pub fn write_report(path: &Path, report: &BenchReport) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating parent dir for {}", path.display()))?;
        }
    }
    let json = serde_json::to_string_pretty(report)?;
    std::fs::write(path, json).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Helper used by the binary so the CLI can pick a default deterministic
/// palace location that lives under `target/`.
pub fn default_palace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/longmemeval_palace")
}

/// Helper used by the binary for the default report path.
pub fn default_report_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/longmemeval_results.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::Turn;

    fn fixture_entry(id: &str, answer_session_idx: usize) -> BenchmarkEntry {
        let sessions: Vec<Vec<Turn>> = vec![
            vec![Turn {
                role: "user".into(),
                content: "I love sourdough bread on weekends".into(),
            }],
            vec![Turn {
                role: "user".into(),
                content:
                    "We migrated the auth provider from Auth0 to Clerk last week and the team is happy"
                        .into(),
            }],
            vec![Turn {
                role: "user".into(),
                content: "I drove to Boston yesterday for a conference".into(),
            }],
        ];
        let session_ids = vec![
            "sess_a".to_string(),
            "sess_b".to_string(),
            "sess_c".to_string(),
        ];
        let answer_id = session_ids[answer_session_idx].clone();
        BenchmarkEntry {
            question_id: id.into(),
            question: "where did I migrate the auth provider to".into(),
            question_type: "single-session-preference".into(),
            question_date: None,
            answer: serde_json::json!("Clerk"),
            answer_session_ids: vec![answer_id],
            haystack_session_ids: session_ids,
            haystack_dates: vec!["d1".into(), "d2".into(), "d3".into()],
            haystack_sessions: sessions,
        }
    }

    #[test]
    fn score_perfect_top1() {
        let retrieved = vec!["sess_b".to_string(), "sess_a".to_string()];
        let truth: HashSet<String> = ["sess_b"].iter().map(|s| s.to_string()).collect();
        let (r5, r10, mrr, rank) = score(&retrieved, &truth);
        assert_eq!(r5, 1.0);
        assert_eq!(r10, 1.0);
        assert!((mrr - 1.0).abs() < 1e-9);
        assert_eq!(rank, Some(1));
    }

    #[test]
    fn score_misses() {
        let retrieved = vec!["sess_x".to_string(), "sess_y".to_string()];
        let truth: HashSet<String> = ["sess_b"].iter().map(|s| s.to_string()).collect();
        let (r5, r10, mrr, rank) = score(&retrieved, &truth);
        assert_eq!(r5, 0.0);
        assert_eq!(r10, 0.0);
        assert_eq!(mrr, 0.0);
        assert_eq!(rank, None);
    }

    #[test]
    fn score_rank_at_3() {
        let retrieved = vec![
            "sess_a".to_string(),
            "sess_x".to_string(),
            "sess_b".to_string(),
            "sess_c".to_string(),
            "sess_d".to_string(),
        ];
        let truth: HashSet<String> = ["sess_b"].iter().map(|s| s.to_string()).collect();
        let (r5, r10, mrr, rank) = score(&retrieved, &truth);
        assert_eq!(r5, 1.0);
        assert_eq!(r10, 1.0);
        assert!((mrr - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(rank, Some(3));
    }

    #[tokio::test]
    async fn run_one_question_end_to_end() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let entry = fixture_entry("q1", 1);
        let entries = vec![entry];

        let mut buf: Vec<u8> = Vec::new();
        let report = run(&entries, tmp.path(), None, &mut buf)
            .await
            .expect("run ok");

        assert_eq!(report.summary.total_questions, 1);
        assert_eq!(report.summary.scored, 1);
        assert_eq!(report.records.len(), 1);

        // The auth-migration session should rank top-1 against the auth
        // query — naive_similarity gives it the highest token overlap.
        let r = &report.records[0];
        assert!(r.scored);
        assert_eq!(r.recall_at_5, 1.0);
        assert_eq!(r.recall_at_10, 1.0);
        assert_eq!(r.correct_first_rank, Some(1));

        // NDJSON sanity: question line + summary line, both valid JSON.
        let text = String::from_utf8(buf).expect("utf8");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "ndjson = {text:?}");
        let q: serde_json::Value = serde_json::from_str(lines[0]).expect("q json");
        assert_eq!(q["type"], "question");
        let s: serde_json::Value = serde_json::from_str(lines[1]).expect("summary json");
        assert_eq!(s["type"], "summary");
    }

    #[tokio::test]
    async fn empty_haystack_skipped() {
        let mut entry = fixture_entry("q-empty", 1);
        // Strip every session of user content.
        for session in &mut entry.haystack_sessions {
            for turn in session.iter_mut() {
                turn.role = "assistant".into();
            }
        }
        let entries = vec![entry];
        let tmp = tempfile::tempdir().unwrap();
        let mut buf: Vec<u8> = Vec::new();
        let report = run(&entries, tmp.path(), None, &mut buf).await.unwrap();
        assert_eq!(report.summary.skipped, 1);
        assert_eq!(report.summary.scored, 0);
    }
}
