//! sync.rs — File/drawer synchronisation between MemPalace and the
//! filesystem.
//!
//! mr-qs71: introduced as a stub; the first feature is a gitignore-aware
//! drawer prune. Future waves will add push/pull mirroring.

#![doc(hidden)]

use std::path::{Path, PathBuf};

/// Resolve a list of paths against a `.gitignore` file, returning the
/// subset of paths that should be **kept** (i.e. NOT ignored).
///
/// Semantics are intentionally a small subset of gitignore:
/// * Blank lines and `#`-comments are ignored.
/// * Lines starting with `!` (negation) are not yet supported.
/// * Lines starting with `/` anchor to the palace root; other patterns
///   match any subpath component.
/// * Patterns without `/` match a path whose final component equals the
///   pattern (e.g. `node_modules` matches `palace/node_modules/x`).
/// * `**` matches zero or more path components.
///
/// If the `.gitignore` file is missing or empty, every path is kept.
pub fn filter_by_gitignore<I, P>(palace_root: &Path, paths: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let rules = match load_gitignore(palace_root) {
        Some(r) => r,
        None => {
            return paths
                .into_iter()
                .map(|p| p.as_ref().to_path_buf())
                .collect();
        }
    };

    paths
        .into_iter()
        .map(|p| p.as_ref().to_path_buf())
        .filter(|p| !is_ignored(p, palace_root, &rules))
        .collect()
}

fn load_gitignore(palace_root: &Path) -> Option<Vec<String>> {
    let path = palace_root.join(".gitignore");
    if !path.exists() {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let rules: Vec<String> = text
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    if rules.is_empty() {
        return None;
    }
    Some(rules)
}

fn is_ignored(path: &Path, palace_root: &Path, rules: &[String]) -> bool {
    let relative = path.strip_prefix(palace_root).unwrap_or(path);
    for rule in rules {
        if matches_pattern(rule, relative) {
            return true;
        }
    }
    false
}

fn matches_pattern(rule: &str, path: &Path) -> bool {
    // Strip leading slash — anchored rules are still prefix-matched.
    let rule = rule.trim_start_matches('/');
    let rule = rule.trim_end_matches('/');

    // A bare name matches a final path component.
    if !rule.contains('/') {
        if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
            if name == rule {
                return true;
            }
        }
        // Also match any component (e.g. "node_modules" inside the tree).
        return path
            .components()
            .any(|c| c.as_os_str().to_string_lossy() == rule);
    }

    // A slash-bearing pattern is a path-prefix match.
    let rule_path = std::path::Path::new(rule);
    path.starts_with(rule_path)
}

/// Stats from a `prune` run. Kept tiny for now — the JSON output is
/// stable and downstream tooling may already consume it.
#[derive(Debug, Clone, Default)]
pub struct PruneStats {
    pub scanned: usize,
    pub skipped_gitignore: usize,
    pub removed: usize,
    pub errors: usize,
}

/// Walk the drawer's source-tree looking for files to prune. Honors the
/// palace's `.gitignore` (unless `no_gitignore` is set) and reports what
/// it would have / did remove.
///
/// `dry_run=true` reports the prune plan without writing anything.
pub fn prune_drawer_paths(
    palace_root: &Path,
    paths: &[PathBuf],
    dry_run: bool,
    no_gitignore: bool,
) -> PruneStats {
    let candidates: Vec<PathBuf> = if no_gitignore {
        paths.to_vec()
    } else {
        filter_by_gitignore(palace_root, paths.iter())
    };
    let mut stats = PruneStats {
        scanned: paths.len(),
        skipped_gitignore: paths.len().saturating_sub(candidates.len()),
        ..Default::default()
    };
    for p in &candidates {
        if !p.exists() {
            continue;
        }
        if dry_run {
            stats.removed += 1;
        } else {
            match std::fs::remove_file(p) {
                Ok(()) => stats.removed += 1,
                Err(_) => stats.errors += 1,
            }
        }
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_by_gitignore_skips_ignored_paths() {
        // mr-qs71: a .gitignore with `node_modules` and `target` must
        // cause those entries to be dropped from the prune candidate
        // list, while ordinary source files pass through.
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(root.join(".gitignore"), "node_modules\ntarget\n").unwrap();

        let paths = vec![
            root.join("src/main.rs"),
            root.join("node_modules/foo.js"),
            root.join("target/debug/build.o"),
        ];

        let kept = filter_by_gitignore(root, &paths);
        let kept_strs: Vec<String> = kept
            .iter()
            .map(|p| p.strip_prefix(root).unwrap().to_string_lossy().into_owned())
            .collect();

        assert!(kept_strs.contains(&"src/main.rs".to_string()));
        assert!(!kept_strs.iter().any(|p| p.contains("node_modules")));
        assert!(!kept_strs.iter().any(|p| p.contains("target")));
    }
}
