//! dedup_window.rs — Online SHA-256 rolling-window dedup (mp-032).
//!
//! Complements the offline near-duplicate sweep in [`crate::dedup`] /
//! [`crate::sweeper`]: while those passes run *across* the palace and use
//! cosine-similarity heuristics, this module is an in-memory primitive that
//! catches *exactly equal* observations within a session before they ever
//! reach disk.
//!
//! Use case: an agent capture pipeline (PostToolUse hooks, Claude Code save
//! hook, etc.) replays the same prompt or tool result several times within a
//! few seconds. Each replay would otherwise become a new drawer. With this
//! filter, only the first observation in the configured window is admitted;
//! subsequent duplicates are silently skipped and logged via
//! `tracing::debug!`.
//!
//! ## Design
//!
//! - **Hash.** `SHA-256` over `content.trim()` so trivial whitespace
//!   variation (`"foo"`, `"  foo  "`, `"\nfoo\t"`) collapses to the same
//!   key. Truncating in `add_drawer` means leading/trailing whitespace
//!   never produces a "different" drawer.
//! - **Storage.** A `HashMap<[u8; 32], Instant>` for O(1) membership +
//!   a `VecDeque<([u8; 32], Instant)>` insert-order log. The deque keeps
//!   purge cheap (front-to-back age-monotonic) and is also the LRU
//!   eviction path when the table reaches `capacity`.
//! - **Concurrency.** A single `std::sync::Mutex` guards the inner state.
//!   The lock is held only for the duration of one `check_and_record`
//!   call (a hash lookup + at most one purge / one push / one pop), so
//!   contention is irrelevant in practice. We deliberately avoid
//!   `parking_lot` to keep the workspace dependency surface stable; the
//!   hot path is so short that fairness/cost differences vs. `parking_lot`
//!   are not measurable.
//!
//! ## Defaults
//!
//! - `DEFAULT_WINDOW = 300 s` — five minutes, matching the mempalace
//!   reference impl (report 06 §3.5).
//! - `DEFAULT_CAPACITY = 4096` — bounds memory at ~256 KiB worst case
//!   (32 B hash + a few bytes for `Instant`).
//!
//! Both are **hardcoded** today. ADR-7 ("`PalaceConfig` shape") wires
//! these into `crates/core/src/config.rs` once it lands; for now use
//! `WindowedDedup::with_window` / `WindowedDedup::new` to override.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

/// Default rolling window length: five minutes (report 06 §3.5).
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(300);

/// Default LRU capacity. Caps memory at ~256 KiB even when a session
/// generates several thousand distinct observations per minute.
pub const DEFAULT_CAPACITY: usize = 4096;

/// Verdict from a single [`WindowedDedup::check_and_record`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DedupVerdict {
    /// Hash was not seen within the rolling window. The caller should
    /// proceed with the insert; the hash has now been recorded.
    Fresh,
    /// Hash matched an entry currently inside the rolling window. The
    /// caller should skip the insert.
    Duplicate,
}

#[derive(Debug, Default)]
struct Inner {
    /// Insert-order log of `(hash, recorded_at)` pairs. Front is the
    /// oldest, so purge is `pop_front` while front is past the window.
    queue: VecDeque<([u8; 32], Instant)>,
    /// Membership index keyed by hash for O(1) duplicate detection.
    seen: HashMap<[u8; 32], Instant>,
}

/// Online SHA-256 dedup with a rolling time window and bounded LRU
/// capacity. See module docs for design notes.
#[derive(Debug)]
pub struct WindowedDedup {
    window: Duration,
    capacity: usize,
    inner: Mutex<Inner>,
}

impl Default for WindowedDedup {
    /// Equivalent to `WindowedDedup::new(DEFAULT_WINDOW, DEFAULT_CAPACITY)`.
    fn default() -> Self {
        Self::new(DEFAULT_WINDOW, DEFAULT_CAPACITY)
    }
}

impl WindowedDedup {
    /// Construct a dedup window with explicit `window` and `capacity`.
    ///
    /// `capacity` is normalised up to `1` (a zero-capacity LRU would
    /// trivially evict every insert and admit every duplicate, which is
    /// almost never what callers want).
    pub fn new(window: Duration, capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            window,
            capacity,
            inner: Mutex::new(Inner {
                queue: VecDeque::with_capacity(capacity),
                seen: HashMap::with_capacity(capacity),
            }),
        }
    }

    /// Convenience: `WindowedDedup::new(window, DEFAULT_CAPACITY)`.
    pub fn with_window(window: Duration) -> Self {
        Self::new(window, DEFAULT_CAPACITY)
    }

    /// Hash the trimmed `content` and either record it (returning
    /// [`DedupVerdict::Fresh`]) or report it as already seen
    /// ([`DedupVerdict::Duplicate`]).
    ///
    /// Side effect on `Fresh`: the hash is inserted, expired entries are
    /// pruned, and if the table is now over `capacity` the oldest entry
    /// is evicted (LRU by insert time).
    pub fn check_and_record(&self, content: &str) -> DedupVerdict {
        self.check_and_record_at(content, Instant::now())
    }

    /// Test-friendly variant of [`Self::check_and_record`] that takes an
    /// explicit `now`. Public-but-hidden so tests in the same crate can
    /// drive deterministic time travel without touching the system clock.
    #[doc(hidden)]
    pub fn check_and_record_at(&self, content: &str, now: Instant) -> DedupVerdict {
        let hash = hash_normalized(content);
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Self::purge_inner(&mut guard, now, self.window);

        if guard.seen.contains_key(&hash) {
            return DedupVerdict::Duplicate;
        }

        guard.seen.insert(hash, now);
        guard.queue.push_back((hash, now));

        // LRU by insert order: evict the oldest until we are within capacity.
        while guard.queue.len() > self.capacity {
            if let Some((old_hash, _)) = guard.queue.pop_front() {
                guard.seen.remove(&old_hash);
            } else {
                break;
            }
        }

        DedupVerdict::Fresh
    }

    /// Drop entries whose recorded time is older than `window`. Cheap
    /// (`O(expired_count)`) because `queue` is age-monotonic.
    pub fn purge_expired(&self) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Self::purge_inner(&mut guard, Instant::now(), self.window);
    }

    fn purge_inner(guard: &mut Inner, now: Instant, window: Duration) {
        while let Some(&(hash, t)) = guard.queue.front() {
            if now.saturating_duration_since(t) > window {
                guard.queue.pop_front();
                guard.seen.remove(&hash);
            } else {
                break;
            }
        }
    }

    /// Number of live entries (after any pending purge has been flushed
    /// by an earlier `check_and_record` / `purge_expired` call).
    pub fn len(&self) -> usize {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.seen.len()
    }

    /// Convenience: `self.len() == 0`.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Configured rolling window length.
    pub fn window(&self) -> Duration {
        self.window
    }

    /// Configured LRU capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

/// Compute SHA-256 over `content.trim()`. Pub-but-hidden so callers (e.g.
/// `palace_db::add_drawer`) can log the hash without re-implementing the
/// normalisation rule.
#[doc(hidden)]
pub fn hash_normalized(content: &str) -> [u8; 32] {
    let normalized = content.trim();
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    /// Two calls with identical content within < 1 s: first Fresh, second
    /// Duplicate. The textbook happy path.
    #[test]
    fn fresh_then_duplicate_within_window() {
        let dedup = WindowedDedup::default();
        assert_eq!(dedup.check_and_record("hello world"), DedupVerdict::Fresh);
        assert_eq!(
            dedup.check_and_record("hello world"),
            DedupVerdict::Duplicate
        );
        assert_eq!(dedup.len(), 1);
    }

    /// Same content at `t = 0` and `t = window + 1 s`: both Fresh. The
    /// first entry must be purged before the second hash check fires.
    #[test]
    fn fresh_again_after_window_expires() {
        let dedup = WindowedDedup::new(Duration::from_secs(60), 16);
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_secs(61);
        assert_eq!(
            dedup.check_and_record_at("hello", t0),
            DedupVerdict::Fresh,
            "first call should be Fresh"
        );
        assert_eq!(
            dedup.check_and_record_at("hello", t1),
            DedupVerdict::Fresh,
            "after window+1s the entry should have been purged"
        );
        assert_eq!(dedup.len(), 1);
    }

    /// Capacity-bound LRU: filling 4097 distinct slots in a 4096-cap
    /// dedup must evict the very first one. We check the still-resident
    /// neighbour first (a `Duplicate` verdict is a pure read; it does
    /// not mutate state), then verify that the oldest entry has been
    /// evicted via a fresh re-insert.
    #[test]
    fn capacity_4097_distinct_evicts_oldest() {
        let dedup = WindowedDedup::new(Duration::from_secs(3600), DEFAULT_CAPACITY);
        let now = Instant::now();
        for i in 0..DEFAULT_CAPACITY {
            assert_eq!(
                dedup.check_and_record_at(&format!("c_{i}"), now),
                DedupVerdict::Fresh
            );
        }
        // 4097th distinct item: forces eviction of c_0 (oldest).
        // Queue ends up as [c_1, c_2, ..., c_4095, c_overflow].
        assert_eq!(
            dedup.check_and_record_at("c_overflow", now),
            DedupVerdict::Fresh
        );
        assert_eq!(dedup.len(), DEFAULT_CAPACITY);

        // c_1 — the new oldest entry — must still be resident. We assert
        // this *first* because a `Duplicate` verdict is a pure read and
        // doesn't perturb the queue, whereas the next assertion will.
        assert_eq!(
            dedup.check_and_record_at("c_1", now),
            DedupVerdict::Duplicate,
            "second-oldest entry should still be live"
        );
        // c_4095 (most recently inserted in the bulk loop) is also still
        // resident.
        assert_eq!(
            dedup.check_and_record_at(&format!("c_{}", DEFAULT_CAPACITY - 1), now),
            DedupVerdict::Duplicate,
            "newest bulk-inserted entry should still be live"
        );
        // And the overflow item itself.
        assert_eq!(
            dedup.check_and_record_at("c_overflow", now),
            DedupVerdict::Duplicate
        );
        // c_0 was evicted by the overflow insert → re-insert is Fresh.
        assert_eq!(
            dedup.check_and_record_at("c_0", now),
            DedupVerdict::Fresh,
            "oldest entry should have been evicted"
        );
    }

    /// Whitespace-only differences must collapse to the same hash so that
    /// `"foo"` and `"  foo  "` (and `"\nfoo\t"`) dedup against each other.
    #[test]
    fn whitespace_normalisation_dedupes() {
        let dedup = WindowedDedup::default();
        assert_eq!(dedup.check_and_record("foo"), DedupVerdict::Fresh);
        assert_eq!(
            dedup.check_and_record("  foo  "),
            DedupVerdict::Duplicate,
            "leading/trailing spaces must trim to the same hash"
        );
        assert_eq!(
            dedup.check_and_record("\nfoo\t"),
            DedupVerdict::Duplicate,
            "leading/trailing newlines/tabs must trim to the same hash"
        );
        // But interior whitespace differences are still distinct content.
        assert_eq!(
            dedup.check_and_record("f o o"),
            DedupVerdict::Fresh,
            "interior whitespace is real content, not normalised"
        );
    }

    /// Race-safety: N threads call `check_and_record` with the same content
    /// at the same time (gated through a `Barrier`); exactly one must win
    /// `Fresh`, the rest must see `Duplicate`.
    #[test]
    fn concurrent_same_content_yields_exactly_one_fresh() {
        const N_THREADS: usize = 32;
        let dedup = Arc::new(WindowedDedup::default());
        let barrier = Arc::new(Barrier::new(N_THREADS));
        let mut handles = Vec::with_capacity(N_THREADS);
        for _ in 0..N_THREADS {
            let d = Arc::clone(&dedup);
            let b = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                b.wait();
                d.check_and_record("racy_observation_content")
            }));
        }
        let mut fresh = 0usize;
        let mut dup = 0usize;
        for h in handles {
            match h.join().expect("worker panicked") {
                DedupVerdict::Fresh => fresh += 1,
                DedupVerdict::Duplicate => dup += 1,
            }
        }
        assert_eq!(fresh, 1, "exactly one thread must win Fresh");
        assert_eq!(dup, N_THREADS - 1, "all other threads must see Duplicate");
        assert_eq!(dedup.len(), 1);
    }

    /// `purge_expired` is reachable as a standalone op (e.g. from a
    /// background timer) and drops only past-window entries.
    #[test]
    fn purge_expired_drops_only_old_entries() {
        let dedup = WindowedDedup::new(Duration::from_secs(60), 16);
        let t0 = Instant::now();
        let _ = dedup.check_and_record_at("a", t0);
        let _ = dedup.check_and_record_at("b", t0 + Duration::from_secs(30));
        assert_eq!(dedup.len(), 2);

        // Trigger a purge as-of t0 + 61s by recording a "c" then asking
        // the same instant: "a" (age 61s) is past the 60s window, "b"
        // (age 31s) is still live, "c" was just inserted.
        let t_late = t0 + Duration::from_secs(61);
        let _ = dedup.check_and_record_at("c", t_late);
        assert_eq!(
            dedup.len(),
            2,
            "only `a` should have been purged by the t_late insert"
        );
    }

    /// A capacity of zero must be normalised to one (otherwise every
    /// insert evicts itself and every check is Fresh, defeating the
    /// purpose of the dedup window).
    #[test]
    fn capacity_zero_is_normalised_to_one() {
        let dedup = WindowedDedup::new(Duration::from_secs(60), 0);
        assert_eq!(dedup.capacity(), 1);
        let now = Instant::now();
        assert_eq!(dedup.check_and_record_at("only", now), DedupVerdict::Fresh);
        assert_eq!(
            dedup.check_and_record_at("only", now),
            DedupVerdict::Duplicate
        );
    }

    /// Hash normalisation determinism: identical trimmed inputs produce
    /// identical 32-byte digests; different inputs produce different
    /// digests (sanity check, not a cryptographic proof).
    #[test]
    fn hash_normalized_is_deterministic_and_distinguishing() {
        let h1 = hash_normalized("foo");
        let h2 = hash_normalized("  foo  ");
        let h3 = hash_normalized("bar");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        assert_eq!(h1.len(), 32);
    }

    /// Window length of `Duration::ZERO` admits every insert as Fresh
    /// (never matches itself in a later call). Useful as a "disabled"
    /// configuration.
    #[test]
    fn zero_window_treats_every_call_as_fresh() {
        let dedup = WindowedDedup::new(Duration::ZERO, 16);
        let t0 = Instant::now();
        assert_eq!(dedup.check_and_record_at("x", t0), DedupVerdict::Fresh);
        assert_eq!(
            dedup.check_and_record_at("x", t0 + Duration::from_nanos(1)),
            DedupVerdict::Fresh
        );
    }
}
