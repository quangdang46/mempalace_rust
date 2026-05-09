//! exporter.rs - Export the palace as a browsable folder of markdown files.

use crate::palace_db::PalaceDb;
use std::collections::HashMap;
use std::path::Path;

fn safe_path_component(name: &str) -> String {
    let result = name
        .chars()
        .map(|c| if "/\\:*?\"<>|".contains(c) { '_' } else { c })
        .collect::<String>()
        .trim()
        .to_string();
    if result.is_empty() {
        "unknown".to_string()
    } else {
        result
    }
}

/// Refuse to write into a path that is itself a symlink.
///
/// Defense-in-depth: a pre-placed symlink at the export target would
/// redirect writes to wherever it points (e.g., system directories).
/// Mirrors the miner's input-side caution.
fn reject_symlink(path: &Path, label: &str) -> anyhow::Result<()> {
    if std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        anyhow::bail!(
            "refusing to export: {} is a symbolic link ({}). \
             Remove the symlink or choose a different output path.",
            label,
            path.display()
        );
    }
    Ok(())
}

/// Open a file for writing, refusing to follow a symlink at the target
/// path.
///
/// Mirrors upstream mempalace `7545238` (Copilot review on #1156): the
/// directory-level `reject_symlink` check leaves a TOCTOU window where a
/// symlink swapped in between create-dir and file-open would still
/// redirect writes. On POSIX we close that window with `O_NOFOLLOW`,
/// which fails the open itself if `path` is a symlink. On Windows we
/// fall back to a `symlink_metadata` pre-check (narrower than no check
/// at all).
fn safe_open_for_write(path: &Path, append: bool) -> anyhow::Result<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true);
        if append {
            opts.append(true);
        } else {
            opts.truncate(true);
        }
        opts.custom_flags(libc::O_NOFOLLOW);
        match opts.open(path) {
            Ok(f) => Ok(f),
            Err(e) => {
                // ELOOP: the target was a symlink (O_NOFOLLOW).
                if e.raw_os_error() == Some(libc::ELOOP) {
                    anyhow::bail!("refusing to write: {} is a symbolic link.", path.display());
                }
                Err(e.into())
            }
        }
    }
    #[cfg(not(unix))]
    {
        if std::fs::symlink_metadata(path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            anyhow::bail!("refusing to write: {} is a symbolic link.", path.display());
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true);
        if append {
            opts.append(true);
        } else {
            opts.truncate(true);
        }
        Ok(opts.open(path)?)
    }
}

pub struct ExportStats {
    pub wings: usize,
    pub rooms: usize,
    pub drawers: usize,
}

pub fn export_palace(palace_path: Option<&Path>, output_dir: &Path) -> anyhow::Result<ExportStats> {
    let config = crate::Config::load()?;
    let palace_path = palace_path.unwrap_or(config.palace_path.as_path());
    let db = PalaceDb::open(palace_path)?;

    let total = db.count();
    if total == 0 {
        println!("  Palace is empty -- nothing to export.");
        return Ok(ExportStats {
            wings: 0,
            rooms: 0,
            drawers: 0,
        });
    }

    reject_symlink(output_dir, "output_dir")?;
    std::fs::create_dir_all(output_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(output_dir) {
            let mode = meta.permissions().mode();
            let _ =
                std::fs::set_permissions(output_dir, std::fs::Permissions::from_mode(mode | 0o700));
        }
    }

    let mut opened_rooms: HashMap<String, bool> = HashMap::new();
    let mut created_wing_dirs: HashMap<String, bool> = HashMap::new();
    let mut wing_room_counts: HashMap<String, HashMap<String, usize>> = HashMap::new();
    let mut total_drawers = 0usize;

    println!("  Streaming {} drawers...", total);

    let mut offset = 0usize;
    while offset < total {
        let batch = db.get_all(None, None, 1000);
        if batch.is_empty() || batch[0].ids.is_empty() {
            break;
        }

        let batch_data: Vec<_> = batch[0]
            .documents
            .iter()
            .zip(batch[0].ids.iter())
            .zip(batch[0].metadatas.iter())
            .map(|((doc, id), meta)| (id.clone(), doc.clone(), meta.clone()))
            .collect();

        for (doc_id, doc, meta) in &batch_data {
            let wing = meta
                .get("wing")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let room = meta
                .get("room")
                .and_then(|v| v.as_str())
                .unwrap_or("general");

            let safe_wing = safe_path_component(wing);
            let wing_dir = output_dir.join(&safe_wing);
            if !created_wing_dirs.contains_key(wing) {
                reject_symlink(&wing_dir, &format!("wing directory '{}'", safe_wing))?;
                std::fs::create_dir_all(&wing_dir)?;
                created_wing_dirs.insert(wing.to_string(), true);
            }

            let room_file = wing_dir.join(format!("{}.md", safe_path_component(room)));
            let key = format!("{}|{}", wing, room);
            let is_new = !opened_rooms.contains_key(&key);

            let mut file = safe_open_for_write(&room_file, true)?;
            use std::io::Write;
            if is_new {
                writeln!(file, "# {} / {}\n", wing, room)?;
                opened_rooms.insert(key, true);
            }

            let source = meta
                .get("source_file")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let filed = meta
                .get("filed_at")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let added_by = meta
                .get("added_by")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            writeln!(file, "## {}\n", doc_id)?;
            writeln!(file, "> {}\n", doc)?;
            writeln!(file, "| Field | Value |")?;
            writeln!(file, "|-------|-------|")?;
            writeln!(file, "| Source | {} |", source)?;
            writeln!(file, "| Filed | {} |", filed)?;
            writeln!(file, "| Added by | {} |\n", added_by)?;
            writeln!(file, "---\n")?;

            *wing_room_counts
                .entry(wing.to_string())
                .or_default()
                .entry(room.to_string())
                .or_default() += 1;
            total_drawers += 1;
        }

        offset += batch_data.len();
    }

    // Write index
    let index_path = output_dir.join("index.md");
    let today = chrono::Local::now().format("%Y-%m-%d");
    let mut index_lines = vec![
        format!("# Palace Export -- {}\n", today),
        "".to_string(),
        "| Wing | Rooms | Drawers |".to_string(),
        "|------|-------|---------|".to_string(),
    ];

    let total_wings = wing_room_counts.len();
    let total_rooms: usize = wing_room_counts.values().map(|r| r.len()).sum();

    let mut wings: Vec<_> = wing_room_counts.keys().collect();
    wings.sort();
    for wing in wings {
        let rooms = wing_room_counts.get(wing).unwrap();
        let drawer_count: usize = rooms.values().sum();
        index_lines.push(format!(
            "| [{}]({}/) | {} | {} |",
            wing,
            safe_path_component(wing),
            rooms.len(),
            drawer_count
        ));
    }

    {
        use std::io::Write;
        let mut f = safe_open_for_write(&index_path, false)?;
        f.write_all(index_lines.join("\n").as_bytes())?;
    }

    println!(
        "\n  Exported {} drawers across {} wings, {} rooms",
        total_drawers, total_wings, total_rooms
    );
    println!("  Output: {}", output_dir.display());

    Ok(ExportStats {
        wings: total_wings,
        rooms: total_rooms,
        drawers: total_drawers,
    })
}

#[allow(dead_code)]
#[allow(clippy::manual_is_multiple_of)]
fn chrono_now_date() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    let secs_per_day: u64 = 86400;
    let days = now.as_secs() / secs_per_day;
    let mut y: u64 = 1970;
    let mut remaining = days;
    while remaining >= 365 {
        let is_leap = (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0);
        let days_in_y = if is_leap { 366 } else { 365 };
        if remaining >= days_in_y {
            remaining -= days_in_y;
            y += 1;
        } else {
            break;
        }
    }
    let days_per_month: [u64; 12] = if (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 1usize;
    for (i, &dpm) in days_per_month.iter().enumerate() {
        if remaining < dpm {
            month = i + 1;
            break;
        }
        remaining -= dpm;
    }
    let day = remaining + 1;
    format!("{:04}-{:02}-{:02}", y, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reject_symlink_allows_regular_path() {
        let temp = tempfile::TempDir::new().unwrap();
        let regular = temp.path().join("regular");
        std::fs::create_dir_all(&regular).unwrap();
        assert!(reject_symlink(&regular, "output_dir").is_ok());
    }

    #[test]
    fn test_reject_symlink_allows_missing_path() {
        let temp = tempfile::TempDir::new().unwrap();
        let missing = temp.path().join("missing");
        // A path that does not exist is fine — `create_dir_all` will create it.
        assert!(reject_symlink(&missing, "output_dir").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn test_reject_symlink_blocks_symlinked_dir() {
        let temp = tempfile::TempDir::new().unwrap();
        let target = temp.path().join("real");
        std::fs::create_dir_all(&target).unwrap();
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let err = reject_symlink(&link, "output_dir").unwrap_err();
        let msg = format!("{}", err);
        assert!(msg.contains("symbolic link"), "unexpected error: {msg}");
        assert!(msg.contains("output_dir"), "unexpected error: {msg}");
    }

    #[test]
    fn test_safe_open_for_write_allows_regular_file() {
        // Sanity: a plain (non-existent) path opens fine.
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("regular.md");
        let mut f = safe_open_for_write(&path, false).expect("regular path should open");
        use std::io::Write;
        f.write_all(b"hello").unwrap();
        drop(f);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
    }

    #[cfg(unix)]
    #[test]
    fn test_safe_open_for_write_blocks_symlinked_file() {
        // Mirrors upstream mempalace 7545238: the per-file open must refuse
        // to follow a symlink at the target path, closing the TOCTOU window
        // that the directory-level reject_symlink check leaves open.
        let temp = tempfile::TempDir::new().unwrap();
        let real_target = temp.path().join("real_target.md");
        // The target need not exist — O_NOFOLLOW fails on the symlink
        // itself before resolution.
        let link = temp.path().join("link.md");
        std::os::unix::fs::symlink(&real_target, &link).unwrap();

        let err = safe_open_for_write(&link, false)
            .expect_err("safe_open_for_write must refuse a symlinked target");
        let msg = format!("{}", err);
        assert!(msg.contains("symbolic link"), "unexpected error: {msg}");

        // The symlink target must not have been created behind us.
        assert!(
            !real_target.exists(),
            "symlink target should not have been created"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_safe_open_for_write_appends_to_regular_file() {
        // The append=true variant used for room files preserves prior
        // content (each drawer in a room writes a new section).
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("room.md");
        std::fs::write(&path, "original\n").unwrap();
        {
            let mut f = safe_open_for_write(&path, true).expect("append open should succeed");
            use std::io::Write;
            f.write_all(b"appended\n").unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "original\nappended\n"
        );
    }
}
