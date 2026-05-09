//! PID file guard for mine operations.
//!
//! Prevents concurrent mine processes by creating a PID file that is checked
//! before starting a mine operation. The PID file contains the process ID and
//! timestamp of the current mine operation.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process;
use thiserror::Error;

/// Error types for PID file operations.
#[derive(Error, Debug)]
pub enum PidGuardError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Mine operation already in progress by PID {pid} (started at {timestamp})")]
    AlreadyRunning { pid: u32, timestamp: String },

    #[error("Failed to parse PID file: {0}")]
    ParseError(String),

    #[error("PID file is stale (process {pid} no longer running)")]
    StalePid { pid: u32 },
}

/// PID file guard that prevents concurrent mine operations.
pub struct MinePidGuard {
    pid_file_path: PathBuf,
    acquired: bool,
}

impl MinePidGuard {
    /// Create a new PID guard for the given palace directory.
    pub fn new(palace_dir: &Path) -> Self {
        let pid_file_path = palace_dir.join(".mine.pid");
        Self {
            pid_file_path,
            acquired: false,
        }
    }

    /// Try to acquire the lock (create PID file).
    /// Returns an error if a mine operation is already running.
    pub fn acquire(&mut self) -> Result<(), PidGuardError> {
        // Ensure parent directory exists
        if let Some(parent) = self.pid_file_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Check if PID file exists
        if self.pid_file_path.exists() {
            // Read the existing PID file
            let content = fs::read_to_string(&self.pid_file_path)?;
            let (pid, timestamp) = self.parse_pid_file(&content)?;

            // Check if the process is still running
            if self.is_process_running(pid) {
                return Err(PidGuardError::AlreadyRunning { pid, timestamp });
            } else {
                // Process is not running, clean up stale PID file
                fs::remove_file(&self.pid_file_path)?;
            }
        }

        // Create new PID file
        let current_pid = process::id();
        let timestamp = chrono::Utc::now().to_rfc3339();
        let content = format!("{}\n{}", current_pid, timestamp);

        let mut file = File::create(&self.pid_file_path)?;
        file.write_all(content.as_bytes())?;

        self.acquired = true;
        Ok(())
    }

    /// Release the lock (remove PID file).
    pub fn release(&mut self) {
        if self.acquired && self.pid_file_path.exists() {
            let _ = fs::remove_file(&self.pid_file_path);
        }
        self.acquired = false;
    }

    /// Parse PID file content.
    fn parse_pid_file(&self, content: &str) -> Result<(u32, String), PidGuardError> {
        let lines: Vec<&str> = content.lines().collect();

        if lines.len() < 2 {
            return Err(PidGuardError::ParseError(
                "PID file must contain at least 2 lines".to_string(),
            ));
        }

        let pid: u32 = lines[0]
            .trim()
            .parse::<u32>()
            .map_err(|e: std::num::ParseIntError| PidGuardError::ParseError(e.to_string()))?;

        let timestamp = lines[1].trim().to_string();

        Ok((pid, timestamp))
    }

    /// Check if a process with the given PID is running.
    #[cfg(unix)]
    fn is_process_running(&self, pid: u32) -> bool {
        // On Unix, use kill with signal 0 to check if process exists
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    #[cfg(windows)]
    fn is_process_running(&self, pid: u32) -> bool {
        use std::process::Command;

        // On Windows, use tasklist to check if process exists
        let output = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid)])
            .output();

        match output {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                stdout.contains(&format!("{}", pid))
            }
            Err(_) => false,
        }
    }

    /// Get the PID file path.
    pub fn pid_file_path(&self) -> &Path {
        &self.pid_file_path
    }

    /// Check if the lock is currently acquired.
    pub fn is_acquired(&self) -> bool {
        self.acquired
    }

    /// Force cleanup of stale PID file (use with caution).
    pub fn force_cleanup(&self) -> Result<(), PidGuardError> {
        if self.pid_file_path.exists() {
            fs::remove_file(&self.pid_file_path)?;
        }
        Ok(())
    }
}

impl Drop for MinePidGuard {
    fn drop(&mut self) {
        self.release();
    }
}

/// RAII-style PID guard that automatically releases on drop.
pub struct ScopedMinePidGuard {
    guard: MinePidGuard,
}

impl ScopedMinePidGuard {
    /// Try to acquire the lock, returning the guard if successful.
    pub fn try_acquire(palace_dir: &Path) -> Result<Self, PidGuardError> {
        let mut guard = MinePidGuard::new(palace_dir);
        guard.acquire()?;
        Ok(Self { guard })
    }

    /// Get the inner guard.
    pub fn inner(&self) -> &MinePidGuard {
        &self.guard
    }
}

impl Drop for ScopedMinePidGuard {
    fn drop(&mut self) {
        self.guard.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_pid_guard_creation() {
        let temp_dir = TempDir::new().unwrap();
        let guard = MinePidGuard::new(temp_dir.path());
        assert!(!guard.is_acquired());
    }

    #[test]
    fn test_acquire_release() {
        let temp_dir = TempDir::new().unwrap();
        let mut guard = MinePidGuard::new(temp_dir.path());

        // Acquire the lock
        guard.acquire().unwrap();
        assert!(guard.is_acquired());
        assert!(guard.pid_file_path().exists());

        // Release the lock
        guard.release();
        assert!(!guard.is_acquired());
        assert!(!guard.pid_file_path().exists());
    }

    #[test]
    fn test_concurrent_acquisition() {
        let temp_dir = TempDir::new().unwrap();
        let mut guard1 = MinePidGuard::new(temp_dir.path());
        let mut guard2 = MinePidGuard::new(temp_dir.path());

        // First guard should acquire successfully
        guard1.acquire().unwrap();

        // Second guard should fail
        let result = guard2.acquire();
        assert!(result.is_err());

        // Release first guard
        guard1.release();

        // Now second guard should succeed
        guard2.acquire().unwrap();
        guard2.release();
    }

    #[test]
    fn test_scoped_guard() {
        let temp_dir = TempDir::new().unwrap();
        {
            let _guard = ScopedMinePidGuard::try_acquire(temp_dir.path()).unwrap();
            assert!(temp_dir.path().join(".mine.pid").exists());
        }
        // Guard should be released when dropped
        assert!(!temp_dir.path().join(".mine.pid").exists());
    }

    #[test]
    fn test_force_cleanup() {
        let temp_dir = TempDir::new().unwrap();
        let mut guard = MinePidGuard::new(temp_dir.path());

        guard.acquire().unwrap();
        assert!(guard.pid_file_path().exists());

        // Force cleanup
        guard.force_cleanup().unwrap();
        assert!(!guard.pid_file_path().exists());
    }
}
