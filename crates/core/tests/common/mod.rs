use std::path::PathBuf;
use tempfile::TempDir;

/// Create a temporary directory structure suitable for tests that need a palace path.
/// Returns both the `TempDir` (keeps the directory alive) and the concrete path.
pub fn create_temp_palace_dir(name: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let palace_path = dir.path().join(name);
    std::fs::create_dir_all(&palace_path).unwrap();
    (dir, palace_path)
}
