//! Git branch-aware memory — port of upstream `branch-aware.ts`.
//!
//! Detects git worktrees, lists them, and associates sessions with branches.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

/// Information about a git worktree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Worktree {
    pub path: String,
    pub branch: String,
    pub is_current: bool,
    pub is_bare: bool,
}

/// Session associated with a specific branch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchSession {
    pub session_id: String,
    pub branch: String,
    pub project: String,
}

/// Detect if the given path is inside a git worktree.
pub fn detect_worktree(project_path: &Path) -> Result<Option<Worktree>> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(project_path)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let toplevel = String::from_utf8_lossy(&out.stdout).trim().to_string();

            let branch_output = Command::new("git")
                .args(["branch", "--show-current"])
                .current_dir(project_path)
                .output();

            let branch = branch_output
                .ok()
                .and_then(|o| if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                } else { None })
                .unwrap_or_else(|| "HEAD".to_string());

            let worktrees = list_worktrees(project_path)?;
            let is_current = worktrees.iter().any(|wt| wt.path == toplevel);

            Ok(Some(Worktree {
                path: toplevel,
                branch,
                is_current,
                is_bare: false,
            }))
        }
        _ => Ok(None),
    }
}

/// List all git worktrees for the given project path.
pub fn list_worktrees(project_path: &Path) -> Result<Vec<Worktree>> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(project_path)
        .output()?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut worktrees = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_branch: Option<String> = None;
    let mut is_bare = false;

    for line in text.lines() {
        if line.starts_with("worktree ") {
            if let Some(path) = current_path.take() {
                worktrees.push(Worktree {
                    path: path.clone(),
                    branch: current_branch.take().unwrap_or_default(),
                    is_current: false,
                    is_bare,
                });
            }
            current_path = Some(line["worktree ".len()..].to_string());
            is_bare = false;
        } else if line.starts_with("branch ") {
            let branch_ref = &line["branch ".len()..];
            // refs/heads/main -> main
            current_branch = branch_ref.rsplit('/').next().map(String::from);
        } else if line.starts_with("detached") {
            current_branch = Some("HEAD".to_string());
        } else if line.starts_with("bare") {
            is_bare = true;
        }
    }

    if let Some(path) = current_path {
        worktrees.push(Worktree {
            path,
            branch: current_branch.unwrap_or_default(),
            is_current: false,
            is_bare,
        });
    }

    // Mark current worktree
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_str = cwd.to_string_lossy().to_string();
        for wt in &mut worktrees {
            if cwd_str.starts_with(&wt.path) {
                wt.is_current = true;
                break;
            }
        }
    }

    Ok(worktrees)
}

/// Find sessions associated with a specific branch.
pub fn branch_sessions<'a>(
    sessions: &'a [crate::types::Session],
    branch: &str,
) -> Vec<&'a crate::types::Session> {
    sessions.iter()
        .filter(|s| s.tags.iter().any(|t| t == &format!("branch:{}", branch)))
        .collect()
}

/// Tag a session with its current branch.
pub fn tag_session_with_branch(session: &mut crate::types::Session, project_path: &Path) -> Result<()> {
    if let Some(wt) = detect_worktree(project_path)? {
        let branch_tag = format!("branch:{}", wt.branch);
        if !session.tags.contains(&branch_tag) {
            session.tags.push(branch_tag);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_branch_sessions_filter() {
        use crate::types::Session;
        let sessions = vec![
            Session {
                id: "s-1".into(), project: "p".into(), cwd: "/tmp".into(),
                started_at: chrono::Utc::now(), ended_at: None,
                status: "active".into(), observation_count: 0,
                model: None, tags: vec!["branch:main".into()],
                first_prompt: None, summary: None, commit_shas: vec![],
                agent_id: None,
            },
            Session {
                id: "s-2".into(), project: "p".into(), cwd: "/tmp".into(),
                started_at: chrono::Utc::now(), ended_at: None,
                status: "active".into(), observation_count: 0,
                model: None, tags: vec!["branch:feature".into()],
                first_prompt: None, summary: None, commit_shas: vec![],
                agent_id: None,
            },
        ];
        let main_sessions = branch_sessions(&sessions, "main");
        assert_eq!(main_sessions.len(), 1);
        assert_eq!(main_sessions[0].id, "s-1");
    }

    #[test]
    fn test_tag_session_with_branch() {
        use crate::types::Session;
        let mut session = Session {
            id: "s-1".into(), project: "p".into(), cwd: "/tmp".into(),
            started_at: chrono::Utc::now(), ended_at: None,
            status: "active".into(), observation_count: 0,
            model: None, tags: vec![],
            first_prompt: None, summary: None, commit_shas: vec![],
            agent_id: None,
        };
        let dir = std::env::temp_dir().join(format!("branch_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Not a git repo, so detect_worktree returns None, no tag added
        let _ = tag_session_with_branch(&mut session, &dir);
        assert!(session.tags.is_empty());
    }

    #[test]
    fn test_list_worktrees_not_git_repo() {
        let dir = std::env::temp_dir().join(format!("wt_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let result = list_worktrees(&dir);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_detect_worktree_not_git_repo() {
        let dir = std::env::temp_dir().join(format!("detect_wt_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let result = detect_worktree(&dir);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_detect_worktree_in_git_repo() {
        let dir = std::env::temp_dir().join(format!("detect_wt_repo_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Command::new("git").args(["init"]).current_dir(&dir).output().unwrap();
        Command::new("git").args(["config", "user.email", "test@test.com"]).current_dir(&dir).output().unwrap();
        Command::new("git").args(["config", "user.name", "Test"]).current_dir(&dir).output().unwrap();

        let result = detect_worktree(&dir).unwrap();
        assert!(result.is_some());
        let wt = result.unwrap();
        assert!(wt.is_current);
        assert_eq!(wt.branch, "master");
    }

    #[test]
    fn test_detect_worktree_with_branch() {
        let dir = std::env::temp_dir().join(format!("detect_wt_branch_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Command::new("git").args(["init"]).current_dir(&dir).output().unwrap();
        Command::new("git").args(["config", "user.email", "test@test.com"]).current_dir(&dir).output().unwrap();
        Command::new("git").args(["config", "user.name", "Test"]).current_dir(&dir).output().unwrap();
        Command::new("git").args(["checkout", "-b", "feature-branch"]).current_dir(&dir).output().unwrap();

        let result = detect_worktree(&dir).unwrap();
        assert!(result.is_some());
        let wt = result.unwrap();
        assert_eq!(wt.branch, "feature-branch");
    }

    #[test]
    fn test_worktree_serialization() {
        let wt = Worktree {
            path: "/tmp/repo".to_string(),
            branch: "main".to_string(),
            is_current: true,
            is_bare: false,
        };
        let json = serde_json::to_string(&wt).unwrap();
        let parsed: Worktree = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.path, "/tmp/repo");
        assert!(parsed.is_current);
    }

    #[test]
    fn test_branch_session_serialization() {
        let bs = BranchSession {
            session_id: "s-1".to_string(),
            branch: "main".to_string(),
            project: "my-project".to_string(),
        };
        let json = serde_json::to_string(&bs).unwrap();
        let parsed: BranchSession = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.session_id, "s-1");
    }
}
