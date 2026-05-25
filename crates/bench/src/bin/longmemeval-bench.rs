//! `longmemeval-bench` — Phase 0 LongMemEval-S reproducer (mp-003).
//!
//! Captures the current Rust port's R@5 / R@10 / MRR against the public
//! 500-question split. The harness:
//!
//! 1. Lazily downloads `longmemeval_s.json` into
//!    `crates/bench/data/longmemeval_s/` on first run (skipped via
//!    `--offline`).
//! 2. For every question, mines its haystack into a fresh palace under
//!    `target/longmemeval_palace/<question_id>/`.
//! 3. Calls `mempalace_core::searcher::search_memories` with `limit=10` —
//!    the same path a CLI/MCP user hits in production.
//! 4. Streams NDJSON to stdout (one line per question + a `summary`
//!    footer) and writes a structured JSON report to
//!    `target/longmemeval_results.json`.
//!
//! See `docs/research/06_phase0_longmemeval_baseline.md` for the recorded
//! baseline numbers.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

use mempalace_bench::dataset::{load_from_file, BenchmarkEntry};
use mempalace_bench::longmemeval_fetch::{
    default_data_dir, ensure_dataset, synthetic_fixture_json, LOCAL_FILE,
};
use mempalace_bench::longmemeval_harness::{
    default_palace_root, default_report_path, run, write_report, HARNESS_VERSION,
};

#[derive(Parser, Debug)]
#[command(
    name = "longmemeval-bench",
    about = "Run LongMemEval-S against the Rust port's searcher::search_memories",
    long_about = "LongMemEval-S reproducer (mp-003). Mines each question's \
                  haystack into a fresh palace, then queries the production \
                  search API with limit=10 to capture R@5 / R@10 / MRR. \
                  Streams NDJSON to stdout; writes a structured JSON report \
                  to --output."
)]
struct Args {
    /// Skip the first-run dataset download. Errors cleanly when the
    /// dataset is missing locally.
    #[arg(long)]
    offline: bool,

    /// Use the built-in 1-question synthetic fixture (no network, no
    /// large dataset). Useful for CI smoke and dev-loop verification.
    #[arg(long)]
    self_test: bool,

    /// Limit the number of questions evaluated. Default: all.
    #[arg(long)]
    limit: Option<usize>,

    /// Override the dataset cache directory.
    #[arg(long)]
    data_dir: Option<PathBuf>,

    /// Override the per-question palace root.
    #[arg(long)]
    palace_root: Option<PathBuf>,

    /// Override the JSON report output path.
    #[arg(long)]
    output: Option<PathBuf>,

    /// Override the dataset JSON path directly (skips fetch entirely).
    #[arg(long)]
    dataset: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    eprintln!(
        "[longmemeval-bench] harness={} offline={} self_test={} limit={:?}",
        HARNESS_VERSION, args.offline, args.self_test, args.limit
    );

    let entries = load_entries(&args).await?;
    eprintln!("[longmemeval-bench] loaded {} questions", entries.len());

    let palace_root = args.palace_root.unwrap_or_else(default_palace_root);
    let report_path = args.output.unwrap_or_else(default_report_path);

    eprintln!(
        "[longmemeval-bench] palace_root={} report={}",
        palace_root.display(),
        report_path.display()
    );

    let stdout = std::io::stdout();
    let mut handle = stdout.lock();

    let report = run(&entries, &palace_root, args.limit, &mut handle).await?;

    drop(handle);

    write_report(&report_path, &report)?;

    eprintln!(
        "[longmemeval-bench] DONE — scored={} skipped={} R@5={:.4} R@10={:.4} MRR={:.4} mean={:.1}ms",
        report.summary.scored,
        report.summary.skipped,
        report.summary.recall_at_5,
        report.summary.recall_at_10,
        report.summary.mrr,
        report.summary.mean_elapsed_ms,
    );

    Ok(())
}

async fn load_entries(args: &Args) -> Result<Vec<BenchmarkEntry>> {
    if args.self_test {
        let entries: Vec<BenchmarkEntry> = serde_json::from_str(synthetic_fixture_json())
            .context("parsing built-in synthetic fixture")?;
        return Ok(entries);
    }

    if let Some(p) = &args.dataset {
        if !p.exists() {
            anyhow::bail!("dataset path does not exist: {}", p.display());
        }
        return load_from_file(p);
    }

    let data_dir = args.data_dir.clone().unwrap_or_else(default_data_dir);
    let path = match ensure_dataset(&data_dir, args.offline).await {
        Ok(p) => p,
        Err(e) => {
            // Friendly hint when the pre-cached file is missing — keeps
            // the "offline / unreachable" branch from the task brief
            // ergonomic.
            anyhow::bail!(
                "{e:#}\n\nHint: place the dataset JSON at {} or pass --dataset/--data-dir.",
                data_dir.join(LOCAL_FILE).display()
            );
        }
    };
    load_from_file(&path)
}
