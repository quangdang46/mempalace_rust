//! LongMemEval-S dataset fetcher (mp-003, Phase 0).
//!
//! Downloads the public 500-question "small" split into a local cache so
//! the bench harness can replay it deterministically. We deliberately
//! point at the HuggingFace mirror rather than the upstream Google Drive
//! link from <https://github.com/xiaowu0162/LongMemEval> because:
//!
//! 1. HF supports anonymous range requests, no captcha.
//! 2. The `xiaowu0162/longmemeval-cleaned` repo on HF is the same JSON
//!    snapshot referenced by the upstream README.
//!
//! The dataset is **never** committed — see `.gitignore`. First-run users
//! pay one ~280 MB download; subsequent runs are local.
//!
//! Re-uses the existing `crate::dataset::BenchmarkEntry` schema so the
//! per-question record matches the LoCoMo/LongMemEval evaluator format.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// Public HF mirror of the cleaned LongMemEval-S split.
pub const HF_MIRROR_URL: &str = "https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/main/longmemeval_s_cleaned.json";

/// File name we save under, regardless of remote path. Kept stable so a
/// re-run can short-circuit when the file is already on disk.
pub const LOCAL_FILE: &str = "longmemeval_s.json";

/// Default on-disk cache directory under the workspace, matching the
/// task brief: `crates/bench/data/longmemeval_s/`.
pub fn default_data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/longmemeval_s")
}

/// Returns the local path to the dataset JSON, downloading it if absent
/// and `offline == false`. When `offline == true` and the file is missing
/// this returns an explanatory error so the caller can exit cleanly per
/// the issue brief ("If the network is offline ... exit-cleanly with a
/// clear message when dataset is missing").
pub async fn ensure_dataset(data_dir: &Path, offline: bool) -> Result<PathBuf> {
    let target = data_dir.join(LOCAL_FILE);

    if target.exists() {
        return Ok(target);
    }

    if offline {
        bail!(
            "LongMemEval-S dataset not found at {} and --offline was passed.\n\
             Re-run without --offline (or pre-download {} to that path).",
            target.display(),
            HF_MIRROR_URL
        );
    }

    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("creating dataset cache dir {}", data_dir.display()))?;

    eprintln!(
        "[longmemeval-bench] dataset not cached — downloading from {}",
        HF_MIRROR_URL
    );

    let response = reqwest::get(HF_MIRROR_URL)
        .await
        .with_context(|| format!("GET {}", HF_MIRROR_URL))?;

    if !response.status().is_success() {
        bail!(
            "download failed with status {} from {}",
            response.status(),
            HF_MIRROR_URL
        );
    }

    let bytes = response.bytes().await.context("reading response body")?;

    // Write atomically via a `.partial` sidecar so an aborted download
    // never produces a half-file that future runs would mistakenly skip.
    let tmp = target.with_extension("partial");
    std::fs::write(&tmp, &bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &target)
        .with_context(|| format!("rename {} -> {}", tmp.display(), target.display()))?;

    eprintln!(
        "[longmemeval-bench] cached {} ({} bytes)",
        target.display(),
        bytes.len()
    );

    Ok(target)
}

/// Tiny synthetic fixture matching the LongMemEval-S schema, used by unit
/// tests and the `--self-test` CLI flag so we can exercise the harness
/// without the network. Two sessions, one question, one ground-truth
/// answer session.
pub fn synthetic_fixture_json() -> &'static str {
    r#"[
      {
        "question_id": "synth-0001",
        "question": "where did I migrate the auth provider to",
        "question_type": "single-session-preference",
        "answer": "Clerk",
        "answer_session_ids": ["sess_001"],
        "haystack_session_ids": ["sess_000", "sess_001"],
        "haystack_dates": ["2025/01/01 (Wed) 09:00", "2025/02/01 (Sat) 09:00"],
        "haystack_sessions": [
          [{"role": "user", "content": "I love sourdough bread on weekends"}],
          [{"role": "user", "content": "We migrated the auth provider from Auth0 to Clerk last week and the team is happy with it."}]
        ]
      }
    ]"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_fixture_parses() {
        let entries: Vec<crate::dataset::BenchmarkEntry> =
            serde_json::from_str(synthetic_fixture_json()).expect("valid fixture json");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].question_id, "synth-0001");
        assert_eq!(entries[0].answer_session_ids, vec!["sess_001".to_string()]);
        assert_eq!(entries[0].haystack_sessions.len(), 2);
    }

    #[tokio::test]
    async fn offline_with_no_cache_errors_clean() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let err = ensure_dataset(tmp.path(), true)
            .await
            .expect_err("should fail offline");
        let msg = format!("{:#}", err);
        assert!(msg.contains("offline"), "msg = {}", msg);
    }

    #[tokio::test]
    async fn offline_returns_existing_cache() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::write(tmp.path().join(LOCAL_FILE), b"[]").unwrap();
        let p = ensure_dataset(tmp.path(), true).await.expect("hits cache");
        assert!(p.ends_with(LOCAL_FILE));
    }
}
