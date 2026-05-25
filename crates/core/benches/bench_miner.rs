//! Criterion bench for `miner::mine` end-to-end throughput (mp-002, Phase 0).
//!
//! Generates a synthetic project directory of `MINER_FILE_COUNT` markdown
//! files (each ~`MINER_FILE_CHARS` characters) and measures the full mine
//! pipeline: scan → chunk → upsert → flush. Throughput is reported in files
//! per iteration so improvements in any inner stage (dedup, chunking, JSON
//! flush) move the headline number.
//!
//! Run with `cargo bench --bench bench_miner`. CI compiles via
//! `cargo bench --no-run`.

use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use mempalace_core::miner;
use tempfile::TempDir;
use tokio::runtime::Runtime;

#[path = "common/mod.rs"]
mod common;

use common::write_project_dir;

/// File count chosen so a single mine() call lands in the 100 ms–few-second
/// window on commodity CI. Combined with `MINER_FILE_CHARS` it produces on
/// the order of low-thousands of drawers — the same order of magnitude as
/// the 5 k-drawer fixture used by the search/db benches, but built through
/// the production code path.
const MINER_FILE_COUNT: usize = 200;
/// ~3.2 KB / file ⇒ ≈ 4 chunks per file at the default `CHUNK_SIZE` of 800.
const MINER_FILE_CHARS: usize = 3_200;

fn bench_miner(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");

    // The project dir is *immutable* across iterations — it gets re-mined
    // into a fresh palace each time. Building it once outside the timed
    // section keeps the bench focused on `mine`.
    let project_dir = TempDir::new().expect("project tempdir");
    let file_count = write_project_dir(project_dir.path(), MINER_FILE_COUNT, MINER_FILE_CHARS)
        .expect("write project dir");
    assert_eq!(file_count, MINER_FILE_COUNT);

    let mut group = c.benchmark_group("miner::mine");
    group.throughput(Throughput::Elements(MINER_FILE_COUNT as u64));
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(8));
    group.sample_size(10);

    group.bench_function("mine_200_files_cold", |b| {
        b.to_async(&rt).iter_batched(
            || {
                // Fresh palace tempdir per iteration so dedup / mtime checks
                // can never short-circuit the mine.
                TempDir::new().expect("palace tempdir")
            },
            |palace_tmp| {
                let project_path = project_dir.path().to_path_buf();
                let palace_path = palace_tmp.path().to_path_buf();
                async move {
                    let result = miner::mine(
                        black_box(project_path.as_path()),
                        black_box(palace_path.as_path()),
                        Some("wing_bench"),
                        None,
                    )
                    .await
                    .expect("mine() should succeed against the fixture");
                    // Touch the result so the optimiser can't strip the call.
                    black_box((
                        result.files_processed,
                        result.chunks_created,
                        result.errors.len(),
                        result.files_skipped_chunk_cap,
                    ));
                    // Hold the palace tempdir alive until the future resolves.
                    black_box(palace_tmp);
                }
            },
            BatchSize::PerIteration,
        );
    });

    group.finish();
}

criterion_group!(benches, bench_miner);
criterion_main!(benches);
