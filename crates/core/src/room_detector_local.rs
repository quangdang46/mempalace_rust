use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomMapping {
    pub name: String,
    pub description: String,
    pub keywords: Vec<String>,
}

static SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "__pycache__",
    ".venv",
    "venv",
    "env",
    "dist",
    "build",
    ".next",
    "coverage",
    "target",
    ".sass_cache",
    ".idea",
    ".vscode",
    ".settings",
];

pub fn get_room_patterns() -> &'static [(&'static str, &'static str)] {
    &[
        ("frontend", "frontend"),
        ("front-end", "frontend"),
        ("front_end", "frontend"),
        ("client", "frontend"),
        ("ui", "frontend"),
        ("views", "frontend"),
        ("components", "frontend"),
        ("pages", "frontend"),
        ("screens", "frontend"),
        ("layouts", "frontend"),
        ("backend", "backend"),
        ("back-end", "backend"),
        ("back_end", "backend"),
        ("server", "backend"),
        ("api", "backend"),
        ("routes", "backend"),
        ("services", "backend"),
        ("controllers", "backend"),
        ("handlers", "backend"),
        ("endpoints", "backend"),
        ("middleware", "backend"),
        ("models", "backend"),
        ("model", "backend"),
        ("database", "backend"),
        ("db", "backend"),
        ("schemas", "backend"),
        ("migrations", "backend"),
        ("seeds", "backend"),
        ("repositories", "backend"),
        ("dal", "backend"),
        ("docs", "documentation"),
        ("doc", "documentation"),
        ("documentation", "documentation"),
        ("wiki", "documentation"),
        ("readme", "documentation"),
        ("notes", "documentation"),
        ("guides", "documentation"),
        ("manual", "documentation"),
        ("design", "design"),
        ("designs", "design"),
        ("mockups", "design"),
        ("wireframes", "design"),
        ("assets", "design"),
        ("images", "design"),
        ("img", "design"),
        ("icons", "design"),
        ("fonts", "design"),
        ("storyboard", "design"),
        ("figma", "design"),
        ("sketch", "design"),
        ("costs", "costs"),
        ("cost", "costs"),
        ("budget", "costs"),
        ("finance", "costs"),
        ("financial", "costs"),
        ("pricing", "costs"),
        ("invoices", "costs"),
        ("accounting", "costs"),
        ("payments", "costs"),
        ("billing", "costs"),
        ("meetings", "meetings"),
        ("meeting", "meetings"),
        ("calls", "meetings"),
        ("meeting_notes", "meetings"),
        ("standup", "meetings"),
        ("minutes", "meetings"),
        ("agenda", "meetings"),
        ("sync", "meetings"),
        ("team", "team"),
        ("staff", "team"),
        ("hr", "team"),
        ("hiring", "team"),
        ("employees", "team"),
        ("people", "team"),
        ("recruiting", "team"),
        ("onboarding", "team"),
        ("research", "research"),
        ("references", "research"),
        ("reading", "research"),
        ("papers", "research"),
        ("studies", "research"),
        ("analysis", "research"),
        ("literature", "research"),
        ("planning", "planning"),
        ("roadmap", "planning"),
        ("strategy", "planning"),
        ("specs", "planning"),
        ("spec", "planning"),
        ("requirements", "planning"),
        ("proposal", "planning"),
        ("backlog", "planning"),
        ("sprints", "planning"),
        ("tests", "testing"),
        ("test", "testing"),
        ("testing", "testing"),
        ("qa", "testing"),
        ("e2e", "testing"),
        ("integration", "testing"),
        ("unit", "testing"),
        ("fixtures", "testing"),
        ("mocks", "testing"),
        ("stubs", "testing"),
        ("scripts", "scripts"),
        ("tools", "scripts"),
        ("utils", "scripts"),
        ("utilities", "scripts"),
        ("bin", "scripts"),
        ("commands", "scripts"),
        ("cli", "scripts"),
        ("config", "configuration"),
        ("configs", "configuration"),
        ("settings", "configuration"),
        ("configuration", "configuration"),
        ("infrastructure", "configuration"),
        ("infra", "configuration"),
        ("deploy", "configuration"),
        ("deployment", "configuration"),
        ("terraform", "configuration"),
        ("ansible", "configuration"),
        ("kubernetes", "configuration"),
        ("k8s", "configuration"),
        ("docker", "configuration"),
        ("ci", "configuration"),
        ("logging", "configuration"),
        ("monitoring", "configuration"),
        ("security", "security"),
        ("auth", "security"),
        ("authentication", "security"),
        ("authorization", "security"),
        ("permissions", "security"),
        ("oauth", "security"),
        ("jwt", "security"),
        ("ssl", "security"),
        ("cryptography", "security"),
        ("secrets", "security"),
        ("performance", "performance"),
        ("optimization", "performance"),
        ("profiling", "performance"),
        ("benchmarking", "performance"),
        ("caching", "performance"),
        ("cdn", "performance"),
        ("analytics", "analytics"),
        ("metrics", "analytics"),
        ("tracking", "analytics"),
        ("events", "analytics"),
        ("dashboards", "analytics"),
        ("reporting", "analytics"),
        ("statistics", "analytics"),
        ("communication", "communication"),
        ("notifications", "communication"),
        ("email", "communication"),
        ("sms", "communication"),
        ("webhooks", "communication"),
        ("chat", "communication"),
        ("messaging", "communication"),
        ("integrations", "integrations"),
        ("integration", "integrations"),
        ("third-party", "integrations"),
        ("external", "integrations"),
        ("apis", "integrations"),
    ]
}

fn build_folder_room_map() -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (folder, room) in get_room_patterns() {
        map.insert(folder.to_lowercase(), room.to_string());
    }
    map
}

fn is_skip_dir(name: &str) -> bool {
    SKIP_DIRS.contains(&name)
}

pub fn detect_rooms_from_folders(project_dir: &Path) -> Vec<RoomMapping> {
    let folder_map = build_folder_room_map();
    let mut found_rooms: HashMap<String, String> = HashMap::new();

    if let Ok(entries) = std::fs::read_dir(project_dir) {
        for entry in entries.flatten() {
            if let Ok(item) = entry.path().metadata() {
                if item.is_dir() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if !is_skip_dir(&name_str) {
                        let name_lower = name_str.to_lowercase().replace([' ', '-'], "_");

                        if let Some(room_name) = folder_map.get(&name_lower) {
                            if !found_rooms.contains_key(room_name) {
                                found_rooms.insert(room_name.clone(), name_str.to_string());
                            }
                        } else if name_str.len() > 2
                            && name_str
                                .chars()
                                .next()
                                .map(|c| c.is_alphabetic())
                                .unwrap_or(false)
                        {
                            let clean = name_lower.replace([' ', '-'], "_");
                            if !found_rooms.contains_key(&clean) {
                                found_rooms.insert(clean.clone(), name_str.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    if let Ok(entries) = std::fs::read_dir(project_dir) {
        for entry in entries.flatten() {
            if let Ok(item) = entry.path().metadata() {
                if item.is_dir() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if !is_skip_dir(&name_str) {
                        if let Ok(subdirs) = std::fs::read_dir(entry.path()) {
                            for subentry in subdirs.flatten() {
                                if let Ok(subitem) = subentry.path().metadata() {
                                    if subitem.is_dir() {
                                        let subname = subentry.file_name();
                                        let subname_str = subname.to_string_lossy();
                                        if !is_skip_dir(&subname_str) {
                                            let subname_lower =
                                                subname_str.to_lowercase().replace([' ', '-'], "_");
                                            if let Some(room_name) = folder_map.get(&subname_lower)
                                            {
                                                if !found_rooms.contains_key(room_name) {
                                                    found_rooms.insert(
                                                        room_name.clone(),
                                                        subname_str.to_string(),
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let mut rooms: Vec<RoomMapping> = found_rooms
        .into_iter()
        .map(|(room_name, original)| RoomMapping {
            name: room_name.clone(),
            description: format!("Files from {}/", original),
            keywords: vec![room_name, original.to_lowercase()],
        })
        .collect();

    if !rooms.iter().any(|r| r.name == "general") {
        rooms.push(RoomMapping {
            name: "general".to_string(),
            description: "Files that don't fit other rooms".to_string(),
            keywords: vec![],
        });
    }

    rooms
}

pub fn detect_rooms_from_files(project_dir: &Path) -> Vec<RoomMapping> {
    let mut keyword_counts: HashMap<String, usize> = HashMap::new();

    walkdir::WalkDir::new(project_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .for_each(|entry| {
            let path = entry.path();
            if path.components().any(|c| {
                if let std::path::Component::Normal(name) = c {
                    is_skip_dir(&name.to_string_lossy())
                } else {
                    false
                }
            }) {
                return;
            }

            let filename = path
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase().replace([' ', '-'], "_"))
                .unwrap_or_default();

            for (keyword, room) in get_room_patterns() {
                if filename.contains(keyword) {
                    *keyword_counts.entry(room.to_string()).or_insert(0) += 1;
                }
            }
        });

    let mut rooms: Vec<RoomMapping> = keyword_counts
        .into_iter()
        .filter(|(_, count)| *count >= 2)
        .map(|(room_name, _)| RoomMapping {
            name: room_name.clone(),
            description: format!("Files related to {}", room_name),
            keywords: vec![room_name],
        })
        .collect();

    rooms.sort_by(|a, b| a.name.cmp(&b.name));
    rooms.truncate(6);

    if rooms.is_empty() {
        rooms.push(RoomMapping {
            name: "general".to_string(),
            description: "All project files".to_string(),
            keywords: vec![],
        });
    }

    rooms
}

pub fn print_proposed_structure(
    project_name: &str,
    rooms: &[RoomMapping],
    total_files: usize,
    source: &str,
) {
    println!();
    println!("{}", "=".repeat(55));
    println!("  MemPalace Init - Local setup");
    println!("{}", "=".repeat(55));
    println!();
    println!("  WING: {}", project_name);
    println!(
        "  ({} files found, rooms detected from {})\n",
        total_files, source
    );

    for room in rooms {
        println!("    ROOM: {}", room.name);
        println!("          {}", room.description);
    }

    println!();
    println!("{}", "-".repeat(55));
}

fn read_line(prompt: &str) -> String {
    print!("{}", prompt);
    io::stdout().flush().ok();
    let mut input = String::new();
    io::stdin().read_line(&mut input).ok();
    input.trim().to_string()
}

pub fn get_user_approval(mut rooms: Vec<RoomMapping>) -> Vec<RoomMapping> {
    println!("  Review the proposed rooms above.");
    println!("  Options:");
    println!("    [enter]  Accept all rooms");
    println!("    [edit]   Remove or rename rooms");
    println!("    [add]    Add a room manually");
    println!();

    let choice = read_line("  Your choice [enter/edit/add]: ").to_lowercase();

    if choice.is_empty() || choice == "y" || choice == "yes" {
        return rooms;
    }

    if choice == "edit" {
        println!("\n  Current rooms:");
        for (i, room) in rooms.iter().enumerate() {
            println!("    {}. {} - {}", i + 1, room.name, room.description);
        }

        let remove_input =
            read_line("\n  Room numbers to REMOVE (comma-separated, or enter to skip): ");

        if !remove_input.is_empty() {
            let to_remove: Vec<usize> = remove_input
                .split(',')
                .filter_map(|x| x.trim().parse::<usize>().ok())
                .filter(|&x| x > 0 && x <= rooms.len())
                .map(|x| x - 1)
                .collect();

            rooms = rooms
                .into_iter()
                .enumerate()
                .filter(|(i, _)| !to_remove.contains(i))
                .map(|(_, r)| r)
                .collect();
        }
    }

    let add_rooms =
        choice == "add" || read_line("\n  Add any missing rooms? [y/N]: ").to_lowercase() == "y";

    if add_rooms {
        loop {
            let new_name = read_line("  New room name (or enter to stop): ")
                .to_lowercase()
                .replace(' ', "_");

            if new_name.is_empty() {
                break;
            }

            let new_desc = read_line(&format!("  Description for '{}': ", new_name));

            rooms.push(RoomMapping {
                name: new_name.clone(),
                description: new_desc,
                keywords: vec![new_name.clone()],
            });
            println!("  Added: {}", new_name);
        }
    }

    rooms
}

pub fn save_config(
    project_dir: &Path,
    project_name: &str,
    rooms: &[RoomMapping],
) -> anyhow::Result<()> {
    #[derive(Serialize)]
    struct Config {
        wing: String,
        rooms: Vec<RoomConfig>,
    }

    #[derive(Serialize)]
    struct RoomConfig {
        name: String,
        description: String,
    }

    let config = Config {
        wing: project_name.to_string(),
        rooms: rooms
            .iter()
            .map(|r| RoomConfig {
                name: r.name.clone(),
                description: r.description.clone(),
            })
            .collect(),
    };

    let json = serde_json::to_string_pretty(&config)?;
    let config_path = project_dir.join("mempalace.json");

    std::fs::write(&config_path, json)?;

    println!();
    println!("  Config saved: {:?}", config_path);
    println!();
    println!("  Next step:");
    println!("    mempalace mine {:?}", project_dir);
    println!();
    println!("{}", "=".repeat(55));

    Ok(())
}

pub fn detect_rooms_local(project_dir: &Path) -> anyhow::Result<Vec<RoomMapping>> {
    let project_name = project_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase().replace([' ', '-'], "_"))
        .unwrap_or_else(|| "project".to_string());

    if !project_dir.exists() {
        anyhow::bail!("Directory not found: {:?}", project_dir);
    }

    let mut rooms = detect_rooms_from_folders(project_dir);
    let source = "folder structure";

    if rooms.len() <= 1 {
        rooms = detect_rooms_from_files(project_dir);
    }

    if rooms.is_empty() {
        rooms = vec![RoomMapping {
            name: "general".to_string(),
            description: "All project files".to_string(),
            keywords: vec![],
        }];
    }

    let file_count = count_files(project_dir);
    print_proposed_structure(&project_name, &rooms, file_count, source);
    let approved_rooms = get_user_approval(rooms);
    save_config(project_dir, &project_name, &approved_rooms)?;

    Ok(approved_rooms)
}

pub fn count_files(project_dir: &Path) -> usize {
    walkdir::WalkDir::new(project_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|path| {
            !path.path().components().any(|c| {
                if let std::path::Component::Normal(name) = c {
                    is_skip_dir(&name.to_string_lossy())
                } else {
                    false
                }
            })
        })
        .count()
}

pub fn detect_room(_file_path: &Path, _content: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_get_room_patterns_not_empty() {
        let patterns = get_room_patterns();
        assert!(!patterns.is_empty());
        assert!(
            patterns.len() >= 70,
            "Should have at least 70 folder patterns"
        );
    }

    #[test]
    fn test_detect_rooms_from_folders_basic() {
        let temp_dir = TempDir::new().unwrap();
        let project_dir = temp_dir.path();

        std::fs::create_dir_all(project_dir.join("src")).unwrap();
        std::fs::create_dir_all(project_dir.join("tests")).unwrap();
        std::fs::create_dir_all(project_dir.join("docs")).unwrap();
        std::fs::write(project_dir.join("README.md"), "").unwrap();

        let rooms = detect_rooms_from_folders(project_dir);
        let room_names: Vec<&str> = rooms.iter().map(|r| r.name.as_str()).collect();

        assert!(room_names.contains(&"src"), "Should detect src folder");
        assert!(
            room_names.contains(&"testing"),
            "Should detect testing from tests"
        );
        assert!(
            room_names.contains(&"documentation"),
            "Should detect documentation from docs"
        );
        assert!(
            room_names.contains(&"general"),
            "Should always have general fallback"
        );
    }

    #[test]
    fn test_detect_rooms_skips_node_modules() {
        let temp_dir = TempDir::new().unwrap();
        let project_dir = temp_dir.path();

        std::fs::create_dir_all(project_dir.join("node_modules")).unwrap();
        std::fs::create_dir_all(project_dir.join("src")).unwrap();

        let rooms = detect_rooms_from_folders(project_dir);
        let room_names: Vec<&str> = rooms.iter().map(|r| r.name.as_str()).collect();

        assert!(room_names.contains(&"src"), "Should detect src folder");
        assert!(
            !room_names.contains(&"node_modules"),
            "Should skip node_modules"
        );
    }

    #[test]
    fn test_detect_rooms_from_files_fallback() {
        let temp_dir = TempDir::new().unwrap();
        let project_dir = temp_dir.path();

        std::fs::write(project_dir.join("backend_api.py"), "api code").unwrap();
        std::fs::write(project_dir.join("backend_models.py"), "models code").unwrap();
        std::fs::write(project_dir.join("frontend_ui.py"), "ui code").unwrap();
        std::fs::write(project_dir.join("test_main.py"), "test code").unwrap();
        std::fs::write(project_dir.join("test_utils.py"), "test code").unwrap();

        let rooms = detect_rooms_from_files(project_dir);
        let room_names: Vec<&str> = rooms.iter().map(|r| r.name.as_str()).collect();

        assert!(
            room_names.contains(&"backend"),
            "Should detect backend from filenames"
        );
        assert!(
            room_names.contains(&"testing"),
            "Should detect testing from filenames"
        );
    }

    #[test]
    fn test_room_mapping_struct() {
        let room = RoomMapping {
            name: "test".to_string(),
            description: "Test room".to_string(),
            keywords: vec!["test".to_string()],
        };

        assert_eq!(room.name, "test");
        assert_eq!(room.description, "Test room");
        assert_eq!(room.keywords.len(), 1);
    }

    #[test]
    fn test_is_skip_dir() {
        assert!(is_skip_dir("node_modules"));
        assert!(is_skip_dir(".git"));
        assert!(is_skip_dir("__pycache__"));
        assert!(!is_skip_dir("src"));
        assert!(!is_skip_dir("my_frontend"));
    }

    #[test]
    fn test_build_folder_room_map() {
        let map = build_folder_room_map();
        assert!(!map.is_empty());
        assert_eq!(map.get("frontend"), Some(&"frontend".to_string()));
        assert_eq!(map.get("backend"), Some(&"backend".to_string()));
        assert_eq!(map.get("docs"), Some(&"documentation".to_string()));
    }
}
