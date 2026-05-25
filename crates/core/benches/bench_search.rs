//! Criterion bench for the production search path (mp-002, Phase 0).
//!
//! Pre-populates a tempdir palace with 5 000 deterministic drawers and
//! measures the cost of a single `searcher::search_memories` call. Throughput
//! is reported per query (`Throughput::Elements(1)`); divide reported time
//! by 5 000 to get per-drawer linear-scan cost.
//!
//! Run with `cargo bench --bench bench_search`. CI keeps these compiling
//! via `cargo bench --no-run` only — the timed runs are local/manual.

use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use mempalace_core::searcher::search_memories;
use tempfile::TempDir;
use tokio::runtime::Runtime;

#[path = "common/mod.rs"]
mod common;

use common::{build_drawers, write_palace, FIXTURE_DRAWER_COUNT};

/// Mix of queries hitting different wings/rooms so the linear scan can't
/// degenerate into the same cached substring match every iteration.
const QUERIES: &[&str] = &[
    "auth migration token rotate",
    "graphql schema postgres index",
    "rate limit retry circuit breaker",
    "billing invoice stripe clerk oauth",
    "deploy rollback feature flag",
];

fn bench_search(c: &mut Criterion) {
    // Build the fixture once — Criterion will re-enter `iter` many times,
    // but the on-disk palace is read-only after `setup`.
    let tmp = TempDir::new().expect("tempdir");
    let palace_path = tmp.path().to_path_buf();
    let drawers = build_drawers(FIXTURE_DRAWER_COUNT);
    write_palace(&palace_path, &drawers).expect("write palace");

    let rt = Runtime::new().expect("tokio runtime");

    let mut group = c.benchmark_group("searcher::search_memories");
    // Each `iter()` call performs one full search — report throughput in
    // queries-per-second so cross-PR diffs are obvious.
    group.throughput(Throughput::Elements(1));
    // Linear-scan over 5 k drawers can be > 100 ms; bump the warm-up and
    // measurement floors so Criterion stops nagging about sample count.
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(20);

    let mut query_idx = 0usize;
    group.bench_function("5k_drawers_top_10", |b| {
        b.to_async(&rt).iter(|| {
            let q = QUERIES[query_idx % QUERIES.len()];
            query_idx = query_idx.wrapping_add(1);
            let palace_path = palace_path.clone();
            async move {
                let resp =
                    search_memories(black_box(q), palace_path.as_path(), None, None, 10, None)
                        .await
                        .expect("search_memories should succeed against the fixture");
                black_box(resp.results.len())
            }
        });
    });

    group.bench_function("5k_drawers_wing_filter", |b| {
        b.to_async(&rt).iter(|| {
            let q = QUERIES[query_idx % QUERIES.len()];
            query_idx = query_idx.wrapping_add(1);
            let palace_path = palace_path.clone();
            async move {
                let resp = search_memories(
                    black_box(q),
                    palace_path.as_path(),
                    Some("wing_kai"),
                    None,
                    10,
                    None,
                )
                .await
                .expect("search_memories should succeed against the fixture");
                black_box(resp.results.len())
            }
        });
    });

    group.finish();
}

criterion_group!(benches, bench_search);
criterion_main!(benches);
