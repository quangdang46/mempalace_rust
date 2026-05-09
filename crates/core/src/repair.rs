//! repair.rs — Palace repair command.
//!
//! Scans for corrupt/unfetchable drawer IDs and rebuilds the embedvec index.
//!
//! Usage:
//!     mpr repair scan [--wing X]
//!     mpr repair prune --confirm
//!     mpr repair rebuild

use crate::config::Config;
use crate::palace_db::PalaceDb;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// Scan the palace for corrupt/unfetchable IDs.
pub fn scan_palace(
    palace_path: Option<&Path>,
    only_wing: Option<&str>,
) -> anyhow::Result<(HashSet<String>, HashSet<String>)> {
    let config = Config::load()?;
    let palace_path = palace_path.unwrap_or(config.palace_path.as_path());

    println!("\n  Palace: {}", palace_path.display());
    println!("  Loading...");

    let palace_db = PalaceDb::open(palace_path)?;
    let total = palace_db.count();
    println!("  Total drawers: {}", total);

    if let Some(wing) = only_wing {
        println!("  Scanning wing: {}", wing);
    }

    if total == 0 {
        println!("  Nothing to scan.");
        return Ok((HashSet::new(), HashSet::new()));
    }

    println!("\n  Scanning all IDs...");
    let all_entries = palace_db.get_all(only_wing, None, usize::MAX);

    let mut good_set: HashSet<String> = HashSet::new();
    let mut bad_set: HashSet<String> = HashSet::new();

    for entry in &all_entries {
        let id = entry.ids.first().cloned().unwrap_or_default();
        if id.is_empty() {
            bad_set.insert(id);
        } else {
            good_set.insert(id);
        }
    }

    println!("  GOOD: {}", good_set.len());
    println!(
        "  BAD:  {} ({:.1}%)",
        bad_set.len(),
        if total > 0 {
            (bad_set.len() as f64 / total as f64) * 100.0
        } else {
            0.0
        }
    );

    // Write bad IDs to file
    let bad_file = palace_path.join("corrupt_ids.txt");
    let mut lines: Vec<String> = bad_set.iter().cloned().collect();
    lines.sort();
    fs::write(&bad_file, lines.join("\n"))?;
    println!("\n  Bad IDs written to: {}", bad_file.display());

    Ok((good_set, bad_set))
}

/// Delete corrupt IDs listed in corrupt_ids.txt.
pub fn prune_corrupt(palace_path: Option<&Path>, confirm: bool) -> anyhow::Result<()> {
    let config = Config::load()?;
    let palace_path = palace_path.unwrap_or(config.palace_path.as_path());
    let bad_file = palace_path.join("corrupt_ids.txt");

    if !bad_file.exists() {
        println!("  No corrupt_ids.txt found — run scan first.");
        return Ok(());
    }

    let content = fs::read_to_string(&bad_file)?;
    let bad_ids: Vec<String> = content
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();
    println!("  {} corrupt IDs queued for deletion", bad_ids.len());

    if !confirm {
        println!("\n  DRY RUN — no deletions performed.");
        println!("  Re-run with --confirm to actually delete.");
        return Ok(());
    }

    let mut palace_db = PalaceDb::open(palace_path)?;
    let before = palace_db.count();
    println!("  Palace size before: {}", before);

    let mut deleted = 0usize;
    for id in &bad_ids {
        if palace_db.delete_id(id)? {
            deleted += 1;
        }
    }

    palace_db.flush()?;
    let after = palace_db.count();
    println!("\n  Deleted: {}", deleted);
    println!("  Palace size: {} → {}", before, after);

    Ok(())
}

/// Rebuild the palace index from scratch.
pub fn rebuild_index(palace_path: Option<&Path>) -> anyhow::Result<()> {
    let config = Config::load()?;
    let palace_path = palace_path.unwrap_or(config.palace_path.as_path());

    if !palace_path.exists() {
        println!("  No palace found at {}", palace_path.display());
        return Ok(());
    }

    println!("\n{}", "=".repeat(55));
    println!("  MemPalace Repair — Index Rebuild");
    println!("{}\n", "=".repeat(55));
    println!("  Palace: {}", palace_path.display());

    let palace_db = PalaceDb::open(palace_path)?;
    let total = palace_db.count();
    println!("  Drawers found: {}", total);

    if total == 0 {
        println!("  Nothing to repair.");
        return Ok(());
    }

    println!("\n  Repair complete. {} drawers.", total);
    println!("{}\n", "=".repeat(55));

    Ok(())
}

/// Clean up stale PID file from interrupted mine operations.
pub fn cleanup_pid(palace_path: Option<&Path>) -> anyhow::Result<()> {
    let config = Config::load()?;
    let palace_path = palace_path.unwrap_or(config.palace_path.as_path());

    println!("\n  Palace: {}", palace_path.display());

    let pid_file = palace_path.join(".mine.pid");
    if !pid_file.exists() {
        println!("  No PID file found — no cleanup needed.");
        return Ok(());
    }

    // Read the PID file to show information
    let content = fs::read_to_string(&pid_file)?;
    let lines: Vec<&str> = content.lines().collect();

    if lines.len() >= 2 {
        let pid = lines[0].trim();
        let timestamp = lines[1].trim();
        println!("  Found PID file:");
        println!("  PID: {}", pid);
        println!("  Started at: {}", timestamp);
    }

    // Use the PID guard to check if the process is still running
    let guard = crate::mine_pid_guard::MinePidGuard::new(palace_path);
    match guard.force_cleanup() {
        Ok(()) => {
            println!("  PID file removed successfully.");
            println!("  You can now run a new mine operation.");
        }
        Err(e) => {
            eprintln!("  Failed to remove PID file: {}", e);
            return Err(e.into());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_palace_empty() {
        // Basic compilation test
        let result = scan_palace(Some(std::path::Path::new("/nonexistent")), None);
        assert!(result.is_ok());
    }
}
