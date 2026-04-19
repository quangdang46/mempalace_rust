//! exporter.rs - Export the palace as a browsable folder of markdown files.

use crate::palace_db::PalaceDb;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

pub struct ExportStats {
    pub wings: usize,
    pub rooms: usize,
    pub drawers: usize,
}

pub fn export_palace(palace_path: Option<&Path>, output_dir: &Path) -> anyhow::Result<ExportStats> {
    let config = crate::Config::load()?;
    let palace_path = palace_path.unwrap_or_else(|| config.palace_path.as_path());
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

            let wing_dir = output_dir.join(safe_path_component(wing));
            if !created_wing_dirs.contains_key(wing) {
                std::fs::create_dir_all(&wing_dir)?;
                created_wing_dirs.insert(wing.to_string(), true);
            }

            let room_file = wing_dir.join(format!("{}.md", safe_path_component(room)));
            let key = format!("{}|{}", wing, room);
            let is_new = !opened_rooms.contains_key(&key);

            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&room_file)?;
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

    std::fs::write(&index_path, index_lines.join("\n"))?;

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
