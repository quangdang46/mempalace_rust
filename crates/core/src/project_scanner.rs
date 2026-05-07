use crate::entity_detector::{
    detect_entities as detect_prose_entities, scan_for_detection, DetectionResult, PersonEntity,
    ProjectEntity,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_DEPTH: usize = 6;
const MAX_COMMITS_PER_REPO: usize = 1000;
const MAX_HEADER_LINES: usize = 20;

const SKIP_DIRS: &[&str] = &[
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
    ".terraform",
    "vendor",
    "target",
    ".mempalace",
    ".cache",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInfo {
    pub name: String,
    pub repo_root: PathBuf,
    pub manifest: Option<String>,
    pub has_git: bool,
    pub total_commits: usize,
    pub user_commits: usize,
    pub is_mine: bool,
}

impl ProjectInfo {
    fn confidence(&self) -> f32 {
        if self.is_mine {
            0.99
        } else if self.has_git && self.total_commits > 0 {
            0.70
        } else {
            0.85
        }
    }

    fn signal(&self) -> String {
        let mut parts = Vec::new();
        if let Some(manifest) = &self.manifest {
            parts.push(manifest.clone());
        }
        if self.has_git {
            if self.is_mine && self.user_commits > 0 {
                parts.push(format!("{} of your commits", self.user_commits));
            } else if self.user_commits > 0 {
                parts.push(format!(
                    "{}/{} yours",
                    self.user_commits, self.total_commits
                ));
            } else {
                parts.push(format!("{} commits (none by you)", self.total_commits));
            }
        }
        if parts.is_empty() {
            "repo".to_string()
        } else {
            parts.join(", ")
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonInfo {
    pub name: String,
    pub total_commits: usize,
    pub emails: HashSet<String>,
    pub repos: HashSet<String>,
}

impl PersonInfo {
    fn confidence(&self) -> f32 {
        if self.total_commits >= 100 || self.repos.len() >= 3 {
            0.99
        } else if self.total_commits >= 20 {
            0.85
        } else {
            0.65
        }
    }

    fn signal(&self) -> String {
        format!(
            "{} commit{} across {} repo{}",
            self.total_commits,
            if self.total_commits == 1 { "" } else { "s" },
            self.repos.len(),
            if self.repos.len() == 1 { "" } else { "s" }
        )
    }
}

fn parse_package_json(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let data: Value = serde_json::from_str(&content).ok()?;
    data.get("name")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
}

fn parse_assignment_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    for quote in ['"', '\''] {
        if !trimmed.starts_with(quote) {
            continue;
        }
        let rest = &trimmed[1..];
        let end = rest.find(quote)?;
        let parsed = rest[..end].trim();
        if !parsed.is_empty() {
            return Some(parsed.to_string());
        }
    }
    None
}

fn parse_name_in_sections(path: &Path, sections: &[&[&str]]) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut current_section: Vec<String> = Vec::new();

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            current_section = line
                .trim_matches(['[', ']'])
                .split('.')
                .map(|part| part.trim().to_string())
                .collect();
            continue;
        }

        let in_target = sections.iter().any(|section| {
            current_section.len() == section.len()
                && current_section
                    .iter()
                    .zip(section.iter())
                    .all(|(left, right)| left == right)
        });
        if !in_target {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "name" {
            continue;
        }
        if let Some(parsed) = parse_assignment_value(value) {
            return Some(parsed);
        }
    }

    None
}

fn parse_pyproject(path: &Path) -> Option<String> {
    parse_name_in_sections(path, &[&["project"], &["tool", "poetry"]])
}

fn parse_cargo(path: &Path) -> Option<String> {
    parse_name_in_sections(path, &[&["package"]])
}

fn parse_gomod(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if let Some(module) = line.strip_prefix("module ") {
            let name = module.trim().rsplit('/').next().unwrap_or(module).trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

fn is_skipped_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    SKIP_DIRS.contains(&name) || (name.starts_with('.') && name != ".git")
}

fn has_git_marker(path: &Path) -> bool {
    let git_path = path.join(".git");
    git_path.is_dir() || git_path.is_file()
}

fn collect_manifest_names(root: &Path, repo_root: &Path) -> Vec<(String, String, PathBuf)> {
    fn visit(
        dir: &Path,
        repo_root: &Path,
        depth: usize,
        found: &mut Vec<(String, String, PathBuf)>,
    ) {
        if depth > MAX_DEPTH {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path != repo_root && has_git_marker(&path) {
                    continue;
                }
                if is_skipped_dir(&path) {
                    continue;
                }
                visit(&path, repo_root, depth + 1, found);
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let project_name = match name {
                "package.json" => parse_package_json(&path),
                "pyproject.toml" => parse_pyproject(&path),
                "Cargo.toml" => parse_cargo(&path),
                "go.mod" => parse_gomod(&path),
                _ => None,
            };
            if let Some(project_name) = project_name {
                found.push((
                    name.to_string(),
                    project_name,
                    path.parent().unwrap_or(repo_root).to_path_buf(),
                ));
            }
        }
    }

    let mut found = Vec::new();
    visit(root, repo_root, 0, &mut found);
    found.sort_by_key(|(manifest_file, _, manifest_dir)| {
        let depth = manifest_dir
            .strip_prefix(repo_root)
            .ok()
            .map(|relative| relative.components().count())
            .unwrap_or(MAX_DEPTH + 1);
        let priority = match manifest_file.as_str() {
            "pyproject.toml" => 0,
            "package.json" => 1,
            "Cargo.toml" => 2,
            "go.mod" => 3,
            _ => 4,
        };
        (depth, priority, manifest_dir.to_string_lossy().to_string())
    });
    found
}

fn run_git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git").arg("-C").arg(repo).args(args).output();
    let Ok(output) = output else {
        return String::new();
    };
    if !output.status.success() {
        return String::new();
    }
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn git_user_identity(repo: &Path) -> (String, String) {
    (
        run_git(repo, &["config", "user.name"]),
        run_git(repo, &["config", "user.email"]),
    )
}

fn global_git_identity() -> (String, String) {
    let name = Command::new("git")
        .args(["config", "--global", "user.name"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default();
    let email = Command::new("git")
        .args(["config", "--global", "user.email"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default();
    (name, email)
}

fn git_authors(repo: &Path) -> Vec<(String, String)> {
    let output = run_git(
        repo,
        &[
            "log",
            &format!("--max-count={MAX_COMMITS_PER_REPO}"),
            "--format=%aN|%aE",
        ],
    );
    output
        .lines()
        .filter_map(|line| line.split_once('|'))
        .map(|(name, email)| (name.trim().to_string(), email.trim().to_string()))
        .collect()
}

fn is_bot(name: &str, email: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let email = email.to_ascii_lowercase();
    name.contains("[bot]")
        || name.starts_with("dependabot")
        || name.starts_with("renovate")
        || name.starts_with("github-actions")
        || name.starts_with("actions-user")
        || name.ends_with("-bot")
        || name.ends_with(" bot")
        || name.starts_with("bot-")
        || name.starts_with("snyk")
        || name.starts_with("greenkeeper")
        || name.starts_with("semantic-release")
        || name.starts_with("allcontributors")
        || name.ends_with("-autoroll")
        || name.starts_with("auto-format")
        || email.contains("bot@")
        || email.contains("-bot@")
        || email.contains("[bot]@")
}

fn looks_like_real_name(name: &str) -> bool {
    if name.is_empty() || !name.contains(' ') {
        return false;
    }
    let parts: Vec<&str> = name.split_whitespace().collect();
    if parts.len() < 2 {
        return false;
    }
    parts
        .first()
        .and_then(|first| first.chars().next())
        .map(char::is_uppercase)
        .unwrap_or(false)
        && parts
            .last()
            .and_then(|last| last.chars().next())
            .map(char::is_uppercase)
            .unwrap_or(false)
}

fn find_git_repos(root: &Path) -> Vec<PathBuf> {
    fn visit(dir: &Path, depth: usize, repos: &mut Vec<PathBuf>) {
        if depth > MAX_DEPTH {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if is_skipped_dir(&path) {
                continue;
            }
            if has_git_marker(&path) {
                repos.push(path.clone());
            }
            visit(&path, depth + 1, repos);
        }
    }

    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut repos = Vec::new();
    if has_git_marker(&root) {
        repos.push(root.clone());
    }
    visit(&root, 1, &mut repos);
    repos.sort();
    repos.dedup();
    repos
}

#[derive(Default)]
struct UnionFind {
    parent: HashMap<String, String>,
}

impl UnionFind {
    fn find(&mut self, key: &str) -> String {
        if let Some(parent) = self.parent.get(key).cloned() {
            if parent == key {
                return parent;
            }
            let root = self.find(&parent);
            self.parent.insert(key.to_string(), root.clone());
            return root;
        }
        self.parent.insert(key.to_string(), key.to_string());
        key.to_string()
    }

    fn union(&mut self, a: &str, b: &str) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra != rb {
            self.parent.insert(ra, rb);
        }
    }
}

fn dedupe_people(all_commits: &[(String, String, String)]) -> HashMap<String, PersonInfo> {
    #[derive(Default)]
    struct Aggregate {
        name_counts: HashMap<String, usize>,
        emails: HashSet<String>,
        repos: HashSet<String>,
        total: usize,
    }

    let mut uf = UnionFind::default();
    for (name, email, _) in all_commits {
        let name_key = format!("name:{name}");
        let email_key = if email.is_empty() {
            name_key.clone()
        } else {
            format!("email:{email}")
        };
        uf.union(&name_key, &email_key);
    }

    let mut components: HashMap<String, Aggregate> = HashMap::new();
    for (name, email, repo) in all_commits {
        let key = uf.find(&format!("name:{name}"));
        let aggregate = components.entry(key).or_default();
        *aggregate.name_counts.entry(name.clone()).or_insert(0) += 1;
        if !email.is_empty() {
            aggregate.emails.insert(email.clone());
        }
        aggregate.repos.insert(repo.clone());
        aggregate.total += 1;
    }

    let mut people = HashMap::new();
    for aggregate in components.into_values() {
        let mut variants: Vec<(String, usize)> = aggregate.name_counts.into_iter().collect();
        variants.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let display = variants
            .iter()
            .map(|(name, _)| name)
            .find(|name| looks_like_real_name(name))
            .cloned()
            .unwrap_or_else(|| {
                variants
                    .first()
                    .map(|(name, _)| name.clone())
                    .unwrap_or_default()
            });
        if !looks_like_real_name(&display) {
            continue;
        }
        people
            .entry(display.clone())
            .and_modify(|existing: &mut PersonInfo| {
                existing.total_commits += aggregate.total;
                existing.emails.extend(aggregate.emails.clone());
                existing.repos.extend(aggregate.repos.clone());
            })
            .or_insert(PersonInfo {
                name: display,
                total_commits: aggregate.total,
                emails: aggregate.emails.clone(),
                repos: aggregate.repos.clone(),
            });
    }
    people
}

fn extract_cwd_from_session(session_file: &Path) -> Option<String> {
    let content = std::fs::read_to_string(session_file).ok()?;
    for line in content.lines().take(MAX_HEADER_LINES) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(cwd) = value.get("cwd").and_then(|value| value.as_str()) else {
            continue;
        };
        if !cwd.is_empty() {
            return Some(cwd.to_string());
        }
    }
    None
}

fn decode_slug_fallback(slug: &str) -> String {
    let stripped = slug.trim_start_matches('-');
    let parts: Vec<&str> = stripped
        .split('-')
        .filter(|part| !part.is_empty())
        .collect();
    parts.last().copied().unwrap_or(slug).to_string()
}

fn resolve_project_name(project_dir: &Path) -> String {
    let mut sessions: Vec<PathBuf> = std::fs::read_dir(project_dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
        })
        .collect();
    sessions.sort_by(|a, b| {
        let a_time = a.metadata().and_then(|meta| meta.modified()).ok();
        let b_time = b.metadata().and_then(|meta| meta.modified()).ok();
        b_time.cmp(&a_time)
    });
    for session in sessions {
        if let Some(cwd) = extract_cwd_from_session(&session) {
            return Path::new(&cwd)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&cwd)
                .to_string();
        }
    }
    project_dir
        .file_name()
        .and_then(|name| name.to_str())
        .map(decode_slug_fallback)
        .unwrap_or_else(|| "unknown".to_string())
}

fn is_claude_projects_root(path: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    for entry in entries.flatten() {
        let project_dir = entry.path();
        let Some(name) = project_dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !project_dir.is_dir() || !name.starts_with('-') {
            continue;
        }
        if std::fs::read_dir(&project_dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| {
                entry.path().is_file()
                    && entry.path().extension().and_then(|ext| ext.to_str()) == Some("jsonl")
            })
        {
            return true;
        }
    }
    false
}

fn scan_claude_projects(root: &Path) -> Vec<ProjectInfo> {
    if !is_claude_projects_root(root) {
        return Vec::new();
    }

    let mut by_name = HashMap::<String, ProjectInfo>::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let project_dir = entry.path();
        let Some(name) = project_dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !project_dir.is_dir() || !name.starts_with('-') {
            continue;
        }

        let sessions: Vec<PathBuf> = std::fs::read_dir(&project_dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
            })
            .collect();
        if sessions.is_empty() {
            continue;
        }

        let project_name = resolve_project_name(&project_dir);
        let project = ProjectInfo {
            name: project_name.clone(),
            repo_root: project_dir.clone(),
            manifest: None,
            has_git: false,
            total_commits: sessions.len(),
            user_commits: sessions.len(),
            is_mine: true,
        };
        match by_name.get(&project_name.to_lowercase()) {
            Some(existing) if existing.user_commits >= project.user_commits => {}
            _ => {
                by_name.insert(project_name.to_lowercase(), project);
            }
        }
    }

    let mut projects: Vec<ProjectInfo> = by_name.into_values().collect();
    projects.sort_by(|a, b| {
        b.user_commits
            .cmp(&a.user_commits)
            .then_with(|| a.name.cmp(&b.name))
    });
    projects
}

pub fn scan(root: &Path) -> (Vec<ProjectInfo>, Vec<PersonInfo>) {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if !root.is_dir() {
        return (Vec::new(), Vec::new());
    }

    let repos = find_git_repos(&root);
    let mut me_name = String::new();
    let mut me_email = String::new();
    if let Some(first_repo) = repos.first() {
        (me_name, me_email) = git_user_identity(first_repo);
    }
    if me_name.is_empty() && me_email.is_empty() {
        (me_name, me_email) = global_git_identity();
    }

    let mut projects = HashMap::<String, ProjectInfo>::new();
    let mut all_commits = Vec::<(String, String, String)>::new();

    for repo in &repos {
        let manifests = collect_manifest_names(repo, repo);
        let (manifest_file, project_name) = manifests
            .first()
            .map(|(file, name, _)| (Some(file.clone()), name.clone()))
            .unwrap_or_else(|| {
                (
                    None,
                    repo.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("unknown")
                        .to_string(),
                )
            });

        let authors = git_authors(repo);
        let non_bot_authors: Vec<(String, String)> = authors
            .into_iter()
            .filter(|(name, email)| !is_bot(name, email))
            .collect();
        let total_commits = non_bot_authors.len();
        let mut user_commits = 0usize;
        let mut author_counts = HashMap::<String, usize>::new();

        for (name, email) in &non_bot_authors {
            *author_counts.entry(name.clone()).or_insert(0) += 1;
            all_commits.push((
                name.clone(),
                email.clone(),
                repo.to_string_lossy().to_string(),
            ));
            if (!me_name.is_empty() && *name == me_name)
                || (!me_email.is_empty() && *email == me_email)
            {
                user_commits += 1;
            }
        }

        let mut top_authors: Vec<(String, usize)> = author_counts
            .iter()
            .map(|(name, count)| (name.clone(), *count))
            .collect();
        top_authors.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let top5: HashSet<String> = top_authors
            .into_iter()
            .take(5)
            .map(|(name, _)| name)
            .collect();

        let is_mine = user_commits > 0
            && ((!me_name.is_empty() && top5.contains(&me_name))
                || (total_commits > 0 && (user_commits as f32 / total_commits as f32) >= 0.10)
                || user_commits >= 20);

        let project = ProjectInfo {
            name: project_name.clone(),
            repo_root: repo.clone(),
            manifest: manifest_file.clone(),
            has_git: true,
            total_commits,
            user_commits,
            is_mine,
        };
        match projects.get(&project_name.to_lowercase()) {
            Some(existing) if existing.user_commits >= project.user_commits => {}
            _ => {
                projects.insert(project_name.to_lowercase(), project);
            }
        }
    }

    if repos.is_empty() {
        for (manifest_file, project_name, _) in collect_manifest_names(&root, &root) {
            projects
                .entry(project_name.to_lowercase())
                .or_insert(ProjectInfo {
                    name: project_name,
                    repo_root: root.clone(),
                    manifest: Some(manifest_file),
                    has_git: false,
                    total_commits: 0,
                    user_commits: 0,
                    is_mine: false,
                });
        }
    }

    let mut project_list: Vec<ProjectInfo> = projects.into_values().collect();
    project_list.sort_by(|a, b| {
        (a.is_mine, a.user_commits, a.total_commits)
            .cmp(&(b.is_mine, b.user_commits, b.total_commits))
            .reverse()
            .then_with(|| a.name.cmp(&b.name))
    });

    let mut people_list: Vec<PersonInfo> = dedupe_people(&all_commits).into_values().collect();
    people_list.sort_by(|a, b| {
        b.total_commits
            .cmp(&a.total_commits)
            .then_with(|| a.name.cmp(&b.name))
    });

    (project_list, people_list)
}

fn merge_detected(
    primary: DetectionResult,
    secondary: DetectionResult,
    drop_secondary_uncertain: bool,
) -> DetectionResult {
    let mut seen = HashSet::<String>::new();
    for name in primary
        .people
        .iter()
        .map(|item| item.name.to_lowercase())
        .chain(primary.projects.iter().map(|item| item.name.to_lowercase()))
        .chain(
            primary
                .uncertain
                .iter()
                .map(|item| item.name.to_lowercase()),
        )
    {
        seen.insert(name);
    }

    let mut merged = primary;
    for person in secondary.people {
        if seen.insert(person.name.to_lowercase()) {
            merged.people.push(person);
        }
    }
    for project in secondary.projects {
        if seen.insert(project.name.to_lowercase()) {
            merged.projects.push(project);
        }
    }
    if !drop_secondary_uncertain {
        for uncertain in secondary.uncertain {
            if seen.insert(uncertain.name.to_lowercase()) {
                merged.uncertain.push(uncertain);
            }
        }
    }
    merged
}

pub fn discover_entities(project_dir: &Path, prose_file_cap: usize) -> DetectionResult {
    let root = project_dir
        .canonicalize()
        .unwrap_or_else(|_| project_dir.to_path_buf());
    let (mut projects, people) = scan(&root);

    if is_claude_projects_root(&root) {
        let mut by_name: HashMap<String, ProjectInfo> = projects
            .into_iter()
            .map(|project| (project.name.to_lowercase(), project))
            .collect();
        for project in scan_claude_projects(&root) {
            let key = project.name.to_lowercase();
            match by_name.get(&key) {
                Some(existing) if existing.user_commits >= project.user_commits => {}
                _ => {
                    by_name.insert(key, project);
                }
            }
        }
        projects = by_name.into_values().collect();
        projects.sort_by(|a, b| {
            (a.is_mine, a.user_commits, a.total_commits)
                .cmp(&(b.is_mine, b.user_commits, b.total_commits))
                .reverse()
                .then_with(|| a.name.cmp(&b.name))
        });
    }

    let real_signal = DetectionResult {
        people: people
            .into_iter()
            .take(15)
            .map(|person| {
                let confidence = person.confidence();
                let context = person.signal();
                PersonEntity {
                    name: person.name,
                    confidence,
                    context,
                }
            })
            .collect(),
        projects: projects
            .into_iter()
            .take(15)
            .map(|project| {
                let confidence = project.confidence();
                let context = project.signal();
                ProjectEntity {
                    name: project.name,
                    confidence,
                    context,
                }
            })
            .collect(),
        uncertain: Vec::new(),
    };

    let prose_files = scan_for_detection(&root, prose_file_cap);
    let prose_detected = if prose_files.is_empty() {
        DetectionResult::default()
    } else {
        detect_prose_entities(&prose_files, prose_file_cap, None)
    };

    let has_real_signal = !real_signal.people.is_empty() || !real_signal.projects.is_empty();
    merge_detected(real_signal, prose_detected, has_real_signal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(path: &Path, args: &[&str], envs: &[(&str, &str)]) {
        let mut command = Command::new("git");
        command.arg("-C").arg(path).args(args);
        for (key, value) in envs {
            command.env(key, value);
        }
        let status = command.status().expect("git command should run");
        assert!(status.success(), "git {:?} failed", args);
    }

    #[test]
    fn test_parse_package_json() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let path = temp.path().join("package.json");
        std::fs::write(&path, r#"{"name":"my-package","version":"1.0.0"}"#).unwrap();
        assert_eq!(parse_package_json(&path).as_deref(), Some("my-package"));
    }

    #[test]
    fn test_parse_pyproject_and_cargo() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let pyproject = temp.path().join("pyproject.toml");
        std::fs::write(&pyproject, "[project]\nname = \"py-app\"\n").unwrap();
        assert_eq!(parse_pyproject(&pyproject).as_deref(), Some("py-app"));

        let cargo = temp.path().join("Cargo.toml");
        std::fs::write(&cargo, "[package]\nname = \"rust-app\"\n").unwrap();
        assert_eq!(parse_cargo(&cargo).as_deref(), Some("rust-app"));
    }

    #[test]
    fn test_find_git_repos_detects_root_and_nested() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        std::fs::create_dir(temp.path().join(".git")).unwrap();
        let nested = temp.path().join("nested");
        std::fs::create_dir_all(nested.join(".git")).unwrap();

        let repos = find_git_repos(temp.path());
        let root = temp
            .path()
            .canonicalize()
            .expect("temp root should canonicalize");
        let nested = nested
            .canonicalize()
            .expect("nested repo path should canonicalize");

        assert!(repos.contains(&root));
        assert!(repos.contains(&nested));
    }

    #[test]
    fn test_discover_entities_prefers_manifest_and_git_people() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let envs = [
            ("GIT_AUTHOR_NAME", "Jane Example"),
            ("GIT_AUTHOR_EMAIL", "jane@example.com"),
            ("GIT_COMMITTER_NAME", "Jane Example"),
            ("GIT_COMMITTER_EMAIL", "jane@example.com"),
        ];

        git(temp.path(), &["init", "-q"], &[]);
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"signal-app\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("notes.md"),
            "Riley: hello there.\nRiley: can you review this?\nHey Riley, thanks for the help.\nRiley laughed again.\n",
        )
        .unwrap();
        git(temp.path(), &["add", "."], &envs);
        git(temp.path(), &["commit", "-q", "-m", "seed"], &envs);

        let detected = discover_entities(temp.path(), 10);
        let projects: Vec<&str> = detected
            .projects
            .iter()
            .map(|entity| entity.name.as_str())
            .collect();
        let people: Vec<&str> = detected
            .people
            .iter()
            .map(|entity| entity.name.as_str())
            .collect();

        assert!(projects.contains(&"signal-app"));
        assert!(people.contains(&"Jane Example"));
        assert!(people.contains(&"Riley"));
    }
}
