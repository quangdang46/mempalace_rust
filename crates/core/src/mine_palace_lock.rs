use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

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
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => match read_lock_pid(&lock_path) {
            Some(pid) if is_process_running(pid) => Err(MineAlreadyRunning { pid }),
            _ => {
                if let Err(e) = fs::remove_file(&lock_path) {
                    return Err(MineAlreadyRunning {
                        pid: e.raw_os_error().unwrap_or(0) as u32,
                    });
                }
                mine_palace_lock(palace_path)
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
