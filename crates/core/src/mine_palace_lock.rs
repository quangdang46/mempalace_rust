use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

#[non_exhaustive]
pub struct MineAlreadyRunning {
    pub pid: u32,
}

impl std::fmt::Debug for MineAlreadyRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MineAlreadyRunning")
            .field("pid", &self.pid)
            .finish()
    }
}

impl std::fmt::Display for MineAlreadyRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "another mpr mine process (PID {}) already running for this palace",
            self.pid
        )
    }
}

impl std::error::Error for MineAlreadyRunning {}

pub fn mine_palace_lock(palace_path: &Path) -> Result<(), MineAlreadyRunning> {
    mine_palace_lock_with_path(palace_path).map(|_| ())
}

/// Like [`mine_palace_lock`] but also returns the lock file path,
/// which the caller can pass to [`release_palace_lock`] after the
/// mine completes. mr-oy1m needs the explicit path so it can release
/// via the same atomic-rename path used by `mine_lock.rs`.
pub fn mine_palace_lock_with_path(palace_path: &Path) -> Result<PathBuf, MineAlreadyRunning> {
    let lock_dir = match get_lock_dir() {
        Ok(d) => d,
        Err(e) => {
            return Err(MineAlreadyRunning {
                pid: e.raw_os_error().unwrap_or(0) as u32,
            });
        }
    };
    if let Err(e) = fs::create_dir_all(&lock_dir) {
        return Err(MineAlreadyRunning {
            pid: e.raw_os_error().unwrap_or(0) as u32,
        });
    }

    let lock_path = lock_dir.join(format!("mine_palace_{}.lock", palace_lock_key(palace_path)));

    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path);

    match file {
        Ok(mut f) => {
            if let Err(e) = writeln!(f, "{}", std::process::id()) {
                return Err(MineAlreadyRunning {
                    pid: e.raw_os_error().unwrap_or(0) as u32,
                });
            }
            if let Err(e) = f.flush() {
                return Err(MineAlreadyRunning {
                    pid: e.raw_os_error().unwrap_or(0) as u32,
                });
            }
            Ok(lock_path)
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => match read_lock_pid(&lock_path) {
            Some(pid) if is_process_running(pid) => Err(MineAlreadyRunning { pid }),
            _ => {
                if let Err(e) = fs::remove_file(&lock_path) {
                    return Err(MineAlreadyRunning {
                        pid: e.raw_os_error().unwrap_or(0) as u32,
                    });
                }
                mine_palace_lock_with_path(palace_path)
            }
        },
        Err(e) => Err(MineAlreadyRunning {
            pid: e.raw_os_error().unwrap_or(0) as u32,
        }),
    }
}

fn get_lock_dir() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "$HOME not set"))?;
    Ok(PathBuf::from(home).join(".mempalace").join("locks"))
}

fn palace_lock_key(palace_path: &Path) -> String {
    let resolved = palace_path
        .canonicalize()
        .unwrap_or_else(|_| palace_path.to_path_buf());
    let lock_key_source = resolved.to_string_lossy().to_lowercase();
    let mut hasher = Sha256::new();
    hasher.update(lock_key_source.as_bytes());
    let hash = hasher.finalize();
    hex::encode(&hash[..8])
}

fn read_lock_pid(path: &PathBuf) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn is_process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let proc_path = format!("/proc/{}", pid);
        if let Ok(metadata) = fs::metadata(&proc_path) {
            let age = std::time::SystemTime::now()
                .duration_since(
                    metadata
                        .modified()
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                )
                .unwrap_or_default()
                .as_secs();
            return age < 60;
        }
    }
    #[cfg(windows)]
    {
        use std::process;
        if pid == process::id() {
            return true;
        }
    }
    false
}

/// mr-jecs: atomic release helper. Renames the lock file to a
/// sibling `.released` sentinel before deletion so a concurrent
/// observer can never see "no lock" + "stale PID" in a window.
///
/// Returns `true` when our own lock was released, `false` if the
/// lock file didn't exist or was held by a different PID.
pub fn release_palace_lock(lock_path: &Path) -> bool {
    let pid_in_file = match read_lock_pid(&lock_path.to_path_buf()) {
        Some(p) => p,
        None => return false,
    };
    if pid_in_file != std::process::id() {
        return false;
    }
    let released = lock_path.with_extension("lock.released");
    if fs::rename(lock_path, &released).is_ok() {
        let _ = fs::remove_file(&released);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir_like() -> std::path::PathBuf {
        let base = std::env::temp_dir();
        let unique = format!(
            "mine_palace_lock_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let p = base.join(unique);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    // mr-jecs: a foreign lock must NOT be released.
    #[cfg(unix)]
    #[test]
    fn test_release_palace_lock_ignores_foreign_pid() {
        let dir = tempdir_like();
        let lock_path = dir.join("foreign_palace.lock");
        // PID we don't own.
        let foreign_pid: u32 = 0xDEAD_BEEF;
        std::fs::write(&lock_path, format!("{}\n", foreign_pid)).unwrap();

        let released = lock_path.with_extension("lock.released");
        let result = release_palace_lock(&lock_path);
        assert!(!result, "foreign lock must not be released");
        assert!(lock_path.exists(), "foreign lock file must remain");
        assert!(!released.exists(), "sentinel must not be created");
        let _ = std::fs::remove_file(&lock_path);
    }

    // mr-jecs: our own lock must be released via the rename path.
    #[cfg(unix)]
    #[test]
    fn test_release_palace_lock_releases_own_pid() {
        let dir = tempdir_like();
        let lock_path = dir.join("our_palace.lock");
        std::fs::write(&lock_path, format!("{}\n", std::process::id())).unwrap();

        let released = lock_path.with_extension("lock.released");
        let result = release_palace_lock(&lock_path);
        assert!(result, "our own lock must be released");
        assert!(!lock_path.exists());
        assert!(!released.exists(), "sentinel must be cleaned up");
    }

    // mr-oy1m: the error struct must carry the holder's PID.
    #[test]
    fn test_mine_already_running_contains_pid() {
        let err = MineAlreadyRunning { pid: 4242 };
        let formatted = format!("{}", err);
        assert!(
            formatted.contains("4242"),
            "Display must include PID, got: {}",
            formatted
        );
        let debug = format!("{:?}", err);
        assert!(
            debug.contains("4242"),
            "Debug must include PID, got: {}",
            debug
        );
    }
}
