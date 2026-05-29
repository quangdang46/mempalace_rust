use anyhow::Result;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::image_refs::ImageRefStore;
use super::image_store::{delete_image, images_dir, is_managed_image_path, max_bytes};

const GRACE_PERIOD_MS: u64 = 30_000; // 30 seconds

pub struct ImageQuotaCleanup {
    image_ref_store: ImageRefStore,
}

#[derive(Debug)]
pub struct CleanupResult {
    pub evicted: u64,
    pub freed_bytes: u64,
    pub under_quota: bool,
}

impl ImageQuotaCleanup {
    pub fn new(image_ref_store: ImageRefStore) -> Self {
        Self { image_ref_store }
    }

    /// Run quota cleanup. Evicts oldest unreferenced images when over quota.
    /// Matches upstream mem::image-quota-cleanup.
    pub fn run(&self) -> Result<CleanupResult> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let images = images_dir();
        if !images.exists() {
            return Ok(CleanupResult {
                evicted: 0,
                freed_bytes: 0,
                under_quota: true,
            });
        }

        // Collect file stats
        let mut total_size: u64 = 0;
        let mut file_stats: Vec<(PathBuf, u64, u64)> = Vec::new();

        for entry in fs::read_dir(&images)? {
            let entry = entry?;
            let path = entry.path();
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();

            // Skip hidden files
            if name.starts_with('.') {
                continue;
            }

            if let Ok(metadata) = fs::metadata(&path) {
                if metadata.is_file() {
                    let size = metadata.len();
                    let mtime = metadata
                        .modified()
                        .unwrap_or(UNIX_EPOCH)
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64;
                    file_stats.push((path, size, mtime));
                    total_size += size;
                }
            }
        }

        let limit = max_bytes();
        if total_size <= limit {
            return Ok(CleanupResult {
                evicted: 0,
                freed_bytes: 0,
                under_quota: true,
            });
        }

        // Sort by mtime (oldest first) for LRU eviction
        file_stats.sort_by_key(|(_, _, mtime)| *mtime);

        let mut total_to_free = total_size - limit;
        let mut evicted = 0;
        let mut freed_bytes = 0;

        for (path, size, mtime) in &file_stats {
            if total_to_free == 0 {
                break;
            }

            // Skip files within grace period
            if now - *mtime < GRACE_PERIOD_MS {
                continue;
            }

            // Check ref count - only evict unreferenced images
            let ref_count = match self.image_ref_store.get_ref_count(path) {
                Ok(count) => count,
                Err(_) => {
                    // Fail-closed: if we can't determine ref count, don't delete
                    continue;
                }
            };

            if ref_count > 0 {
                continue;
            }

            let deleted = delete_image(path)?;
            if deleted > 0 {
                total_to_free = total_to_free.saturating_sub(deleted);
                freed_bytes += deleted;
                evicted += 1;
            }
        }

        Ok(CleanupResult {
            evicted,
            freed_bytes,
            under_quota: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn test_cleanup() -> ImageQuotaCleanup {
        let conn = Connection::open_in_memory().unwrap();
        let store = ImageRefStore::new(conn).unwrap();
        ImageQuotaCleanup::new(store)
    }

    #[test]
    fn test_cleanup_empty_dir() {
        let cleanup = test_cleanup();
        let result = cleanup.run().unwrap();
        assert_eq!(result.evicted, 0);
        assert!(result.under_quota);
    }

    #[test]
    fn test_grace_period_constant() {
        assert_eq!(GRACE_PERIOD_MS, 30_000);
    }

    #[test]
    fn test_cleanup_result_fields() {
        let result = CleanupResult {
            evicted: 5,
            freed_bytes: 1024,
            under_quota: false,
        };
        assert_eq!(result.evicted, 5);
        assert_eq!(result.freed_bytes, 1024);
        assert!(!result.under_quota);
    }
}
