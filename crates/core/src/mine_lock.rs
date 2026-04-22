//! mine_lock.rs — Cross-platform file lock for mine operations.
//!
//! Prevents multiple agents from mining the same file simultaneously,
//! which causes duplicate drawers when the delete+insert cycle interleaves.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;

use sha2::{Digest, Sha256};

/// MineLock provides exclusive access to mine operations for a given source file.
///
/// Uses a lock file on disk (with PID stored inside) to coordinate across processes.
/// The lock is released when the MineLock instance is dropped.
pub struct MineLock {
    #[allow(dead_code)]
    source_file: String,
    lock_path: PathBuf,
}

impl MineLock {
    /// Acquire an exclusive lock on the given source file path.
    ///
    /// Uses SHA256 hash of the source file path to generate a deterministic
    /// lock file name, so the same source file always maps to the same lock.
    ///
    /// # Errors
    ///
    /// Returns an error if the lock cannot be acquired (e.g., another process
    /// holds it).
    pub fn acquire(source_file: &str) -> io::Result<Self> {
        let lock_dir = get_lock_dir()?;
        fs::create_dir_all(&lock_dir)?;

        let lock_path = lock_dir.join(format!("{}.lock", lock_name(source_file)));

        // Try to acquire the lock by writing our PID to the lock file
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path);

        match file {
            Ok(mut f) => {
                // Successfully created lock file - we own it
                writeln!(f, "{}", std::process::id())?;
                f.flush()?;
                Ok(Self {
                    source_file: source_file.to_string(),
                    lock_path,
                })
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                // Lock file exists - check if the process is still running
                match read_lock_pid(&lock_path) {
                    Some(pid) if is_process_running(pid) => {
                        Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            format!("another process (PID {}) holds the lock", pid),
                        ))
                    }
                    _ => {
                        // Stale lock - remove and retry
                        fs::remove_file(&lock_path)?;
                        Self::acquire(source_file)
                    }
                }
            }
            Err(e) => Err(e),
        }
    }
}

impl Drop for MineLock {
    fn drop(&mut self) {
        // Clean up our lock file if it still exists and contains our PID
        if let Ok(mut f) = OpenOptions::new().read(true).open(&self.lock_path) {
            use std::io::Read;
            let mut contents = String::new();
            if f.read_to_string(&mut contents).is_ok() {
                if let Ok(pid) = contents.trim().parse::<u32>() {
                    if pid == std::process::id() {
                        let _ = fs::remove_file(&self.lock_path);
                    }
                }
            }
        }
    }
}

fn get_lock_dir() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "$HOME not set"))?;
    Ok(PathBuf::from(home).join(".mempalace").join("locks"))
}

fn lock_name(source_file: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_file.as_bytes());
    let hash = hasher.finalize();
    hex::encode(&hash[..8])
}

fn read_lock_pid(path: &PathBuf) -> Option<u32> {
    fs::read_to_string(path)
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn is_process_running(pid: u32) -> bool {
    // Simple cross-platform check: try to access /proc/{pid} on Unix-like systems.
    // For simplicity, we use a basic heuristic: if the process dir is young (< 60s),
    // assume the lock is valid. A more robust implementation would use
    // platform-specific APIs.
    let proc_path = format!("/proc/{}", pid);
    if let Ok(metadata) = fs::metadata(&proc_path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            // If modified within last 60 seconds, consider it valid
            let age = std::time::SystemTime::now()
                .duration_since(metadata.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH))
                .unwrap_or_default()
                .as_secs();
            age < 60
        }
        #[cfg(not(unix))]
        {
            let _ = metadata; // suppress warning
            true // Simplified: assume valid on other platforms
        }
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lock_name_deterministic() {
        let name1 = lock_name("/path/to/file.txt");
        let name2 = lock_name("/path/to/file.txt");
        assert_eq!(name1, name2);
        assert_eq!(name1.len(), 16); // 8 bytes = 16 hex chars
    }

    #[test]
    fn test_lock_name_different_paths() {
        let name1 = lock_name("/path/to/file1.txt");
        let name2 = lock_name("/path/to/file2.txt");
        assert_ne!(name1, name2);
    }
}
