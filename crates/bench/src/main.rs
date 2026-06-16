use anyhow::Result;
use clap::{Parser, ValueEnum};
use std::path::PathBuf;

use mempalace_bench::dataset::{
    download_from_huggingface, load_from_file, multilingual_sample, sample_to_entry,
    is_supported_language, Granularity, LOMEMEVAL_FILE,
};
use mempalace_bench::runner::{run_benchmark, BenchmarkConfig};

#[derive(Parser, Debug)]
#[command(
    name = "mempalace-bench",
    about = "Run MemPalace benchmark on LongMemEval dataset"
)]
struct Args {
    /// Path to LongMemEval JSON file.
    /// If not provided, downloads from HuggingFace to cache.
    #[arg(default_value = None)]
    dataset_path: Option<PathBuf>,

    /// Granularity: session (one doc per session) or turn (one doc per user turn).
    #[arg(short, long, default_value = "session")]
    granularity: GranularityArg,

    /// Limit number of questions to run (for quick testing).
    /// By default runs all questions.
    #[arg(short, long)]
    limit: Option<usize>,

    /// Number of results to retrieve from vector DB.
    #[arg(long, default_value = "50")]
    n_results: usize,

    /// Comma-separated list of K values for recall metrics (e.g., "5,10,20").
    #[arg(long, default_value = "5,10")]
    ks: String,

    /// Force download from HuggingFace even if cached.
    #[arg(long, short)]
    download: bool,

    /// Embedding model name (for EmbeddingDb).
    #[arg(long, default_value = "all-MiniLM-L6-v2")]
    embed_model: String,

    /// HuggingFace cache directory.
    #[arg(long, default_value = None)]
    cache_dir: Option<PathBuf>,

    /// mr-d4k3: ISO-639-1 code for a multilingual sample (de/fr/hi/it/ko/ru).
    /// When set, the bench runs the multilingual sample instead of the
    /// English LongMemEval file.
    #[arg(long, default_value = None)]
    language: Option<String>,

    /// mr-d4k3: number of context documents to include per question
    /// (controls the corpus size for the multilingual samples, which
    /// ship a fixed small set; this flag scales the bench up by
    /// repeating the sample for stress tests).
    #[arg(long, default_value = "1")]
    num_ctx: usize,
}

#[derive(ValueEnum, Clone, Copy, Debug, Default)]
enum GranularityArg {
    #[default]
    Session,
    Turn,
}

impl From<GranularityArg> for Granularity {
    fn from(val: GranularityArg) -> Self {
        match val {
            GranularityArg::Session => Granularity::Session,
            GranularityArg::Turn => Granularity::Turn,
        }
    }
}

fn parse_ks(s: &str) -> Vec<usize> {
    s.split(',')
        .map(|part| part.trim().parse().expect("Invalid K value"))
        .collect()
}

fn default_cache_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".cache/mempalace")
    } else {
        PathBuf::from(".cache/mempalace")
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let cache_dir = args.cache_dir.unwrap_or_else(default_cache_dir);

    // mr-d4k3: --language short-circuits the LongMemEval load. We
    // build a synthetic benchmark entry from the multilingual sample
    // and repeat it `num_ctx` times to give the retriever more
    // context to sift through.
    let entries = if let Some(lang) = &args.language {
        if !is_supported_language(lang) {
            anyhow::bail!(
                "unsupported language {:?}; supported: de, fr, hi, it, ko, ru",
                lang
            );
        }
        let sample = multilingual_sample(lang)
            .ok_or_else(|| anyhow::anyhow!("no multilingual sample for {lang}"))?;
        let entries: Vec<_> = (0..args.num_ctx.max(1))
            .map(|i| {
                let mut e = sample_to_entry(&sample);
                e.question_id = format!("{lang}-q{}", i + 1);
                e
            })
            .collect();
        println!(
            "Loaded {} multilingual ({}) sample(s)",
            entries.len(),
            lang
        );
        entries
    } else if let Some(path) = &args.dataset_path {
        if !path.exists() {
            anyhow::bail!("Dataset file not found: {:?}", path);
        }
        println!("Loading dataset from {:?} ...", path);
        load_from_file(path)?
    } else {
        let file_path = if args.download {
            download_from_huggingface(&cache_dir).await?
        } else {
            let file_path = cache_dir.join(LOMEMEVAL_FILE);
            if file_path.exists() {
                file_path
            } else {
                download_from_huggingface(&cache_dir).await?
            }
        };
        println!("Loading dataset from {:?} ...", file_path);
        load_from_file(&file_path)?
    };

    let total = entries.len();
    println!("Loaded {} benchmark entries", total);

    // Parse ks
    let ks = parse_ks(&args.ks);
    println!("Metrics at K: {:?}", ks);
    println!("Granularity: {:?}", args.granularity);

    let config = BenchmarkConfig::new(
        args.granularity.into(),
        args.n_results,
        ks,
        args.limit,
        args.embed_model,
    );

    println!("Running benchmark ...\n");

    let start = std::time::Instant::now();
    let results = run_benchmark(&entries, &config).await?;
    let elapsed = start.elapsed();

    println!("\n{}", results.summary());
    println!("\nTotal time: {:.2}s", elapsed.as_secs_f64());
    println!(
        "Questions/sec: {:.1}",
        results.total_questions as f64 / elapsed.as_secs_f64()
    );

    // Print per-type breakdown if we have multiple types
    if !results.per_type_results.is_empty() && results.per_type_results.len() > 1 {
        println!("\n--- Per-type breakdown ---");
        for (qtype, metrics) in &results.per_type_results {
            let means = metrics.mean();
            if !means.is_empty() {
                println!("\n[qtype: {}]", qtype);
                for (key, val) in &means {
                    println!("  {}: {:.4}", key, val);
                }
            }
        }
    }

    Ok(())
}
