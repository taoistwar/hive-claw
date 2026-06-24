//! Git-backed version control for memory files (port of
//! `nanobot.utils.gitstore`).
//!
//! The Python version uses dulwich; we keep the same API but delegate to
//! the `git` CLI via `std::process::Command` since it is ubiquitous,
//! side-steps the C-bindings / MSRV headaches of `git2`, and avoids the
//! churn of `gix`'s still-evolving surface for porcelain operations like
//! `diff` / `annotate` / `revert`.
//!
//! Functions are best-effort: on any failure they log a warning and return
//! `None` (matching the Python behaviour).

use std::path::{Path, PathBuf};
use std::process::Command;

use log::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct CommitInfo {
    /// Short SHA (8 chars).
    pub sha: String,
    pub message: String,
    /// Formatted datetime, e.g. `2025-01-31 09:12`.
    pub timestamp: String,
}

impl CommitInfo {
    /// Format this commit for display, optionally with a diff.
    pub fn format(&self, diff: &str) -> String {
        let first_line = self.message.lines().next().unwrap_or("");
        let header = format!("## {}\n`{}` \u{2014} {}\n", first_line, self.sha, self.timestamp);
        if !diff.is_empty() {
            format!("{header}\n```diff\n{diff}\n```")
        } else {
            format!("{header}\n(no file changes)")
        }
    }
}

/// Age of a single line based on git blame.
#[derive(Debug, Clone, Copy)]
pub struct LineAge {
    /// Days since last modification.
    pub age_days: i64,
}

/// Git-backed version control for memory files.
#[derive(Debug, Clone)]
pub struct GitStore {
    workspace: PathBuf,
    tracked_files: Vec<String>,
}

impl GitStore {
    pub fn new<P: Into<PathBuf>>(workspace: P, tracked_files: Vec<String>) -> Self {
        Self {
            workspace: workspace.into(),
            tracked_files,
        }
    }

    pub fn is_initialized(&self) -> bool {
        self.workspace.join(".git").is_dir()
    }

    fn git(&self) -> Command {
        let mut cmd = Command::new("git");
        cmd.current_dir(&self.workspace);
        cmd.env("GIT_AUTHOR_NAME", "nanobot");
        cmd.env("GIT_AUTHOR_EMAIL", "nanobot@dream");
        cmd.env("GIT_COMMITTER_NAME", "nanobot");
        cmd.env("GIT_COMMITTER_EMAIL", "nanobot@dream");
        cmd
    }

    fn git_output(&self, args: &[&str]) -> Option<String> {
        match self.git().args(args).output() {
            Ok(out) if out.status.success() => {
                Some(String::from_utf8_lossy(&out.stdout).into_owned())
            }
            Ok(out) => {
                debug!(
                    "git {args:?} failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
                None
            }
            Err(e) => {
                debug!("git {args:?} spawn error: {e}");
                None
            }
        }
    }

    /// Initialise a git repo if not already initialised.
    ///
    /// Creates `.gitignore` and makes an initial commit. Returns `true`
    /// when a new repo was created.
    pub fn init(&self) -> bool {
        if self.is_initialized() {
            return false;
        }
        if self.is_inside_git_repo() {
            warn!(
                "Workspace {} is already inside a git repo; skipping nested repo initialization",
                self.workspace.display()
            );
            return false;
        }
        if self.git().arg("init").output().ok().map(|o| o.status.success()) != Some(true) {
            warn!("Git store init failed for {}", self.workspace.display());
            return false;
        }
        let gitignore_path = self.workspace.join(".gitignore");
        let dream_entries = self.build_gitignore();
        if gitignore_path.exists() {
            if let Ok(existing) = std::fs::read_to_string(&gitignore_path) {
                let existing_lines: std::collections::HashSet<&str> =
                    existing.lines().collect();
                let new_lines: Vec<&str> = dream_entries
                    .lines()
                    .filter(|l| !existing_lines.contains(l))
                    .collect();
                if !new_lines.is_empty() {
                    let merged = format!(
                        "{}\n{}\n",
                        existing.trim_end_matches('\n'),
                        new_lines.join("\n")
                    );
                    let _ = std::fs::write(&gitignore_path, merged);
                }
            }
        } else {
            let _ = std::fs::write(&gitignore_path, &dream_entries);
        }

        // Touch tracked files.
        for rel in &self.tracked_files {
            let p = self.workspace.join(rel);
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if !p.exists() {
                let _ = std::fs::write(&p, "");
            }
        }

        let mut add_args = vec!["add", ".gitignore"];
        for f in &self.tracked_files {
            add_args.push(f.as_str());
        }
        if self.git().args(&add_args).output().ok().map(|o| o.status.success()) != Some(true) {
            warn!("Git store init: initial add failed");
            return true;
        }
        let _ = self
            .git()
            .args(["commit", "-m", "init: nanobot memory store", "--allow-empty"])
            .output();
        info!("Git store initialized at {}", self.workspace.display());
        true
    }

    /// Stage tracked memory files and commit if there are changes.
    ///
    /// Returns the short commit SHA, or `None` if nothing to commit.
    pub fn auto_commit(&self, message: &str) -> Option<String> {
        if !self.is_initialized() {
            return None;
        }
        let status = self.git_output(&["status", "--porcelain"])?;
        if status.trim().is_empty() {
            return None;
        }
        let mut add_args = vec!["add"];
        for f in &self.tracked_files {
            add_args.push(f.as_str());
        }
        if self.git().args(&add_args).output().ok().map(|o| o.status.success()) != Some(true) {
            warn!("Git auto-commit add failed: {message}");
            return None;
        }
        // Check if anything is staged.
        let diff_cached = self
            .git_output(&["diff", "--cached", "--name-only"])
            .unwrap_or_default();
        if diff_cached.trim().is_empty() {
            return None;
        }
        if self
            .git()
            .args(["commit", "-m", message])
            .output()
            .ok()
            .map(|o| o.status.success())
            != Some(true)
        {
            warn!("Git auto-commit failed: {message}");
            return None;
        }
        let sha = self.git_output(&["rev-parse", "--short=8", "HEAD"])?;
        let sha = sha.trim().to_string();
        debug!("Git auto-commit: {sha} ({message})");
        Some(sha)
    }

    fn is_inside_git_repo(&self) -> bool {
        let Ok(current) = self.workspace.canonicalize() else {
            return false;
        };
        let mut current: Option<&Path> = Some(&current);
        while let Some(p) = current {
            if p.join(".git").exists() {
                // The `.git` entry exists under self.workspace too when the
                // repo is already initialized; callers should have gated
                // on `is_initialized()` first. We only want to flag a
                // *parent* repo here.
                if p != self.workspace {
                    return true;
                }
            }
            current = p.parent();
        }
        false
    }

    fn build_gitignore(&self) -> String {
        use std::collections::BTreeSet;
        let mut dirs: BTreeSet<String> = BTreeSet::new();
        for f in &self.tracked_files {
            let p = Path::new(f);
            if let Some(parent) = p.parent() {
                let s = parent.to_string_lossy();
                if !s.is_empty() && s != "." {
                    dirs.insert(s.into_owned());
                }
            }
        }
        let mut lines = vec!["/*".to_string()];
        for d in &dirs {
            lines.push(format!("!{d}/"));
        }
        for f in &self.tracked_files {
            lines.push(format!("!{f}"));
        }
        lines.push("!.gitignore".into());
        let mut out = lines.join("\n");
        out.push('\n');
        out
    }

    /// Return the simplified commit log (up to `max_entries`).
    pub fn log(&self, max_entries: usize) -> Vec<CommitInfo> {
        if !self.is_initialized() {
            return Vec::new();
        }
        // `%h` prints an abbreviated SHA. Use a custom separator that is
        // very unlikely to appear in commit messages.
        let limit = format!("-n{max_entries}");
        let args = ["log", &limit, "--pretty=format:%h\x1f%ci\x1f%s"];
        let Some(out) = self.git_output(&args) else {
            return Vec::new();
        };
        out.lines()
            .filter(|l| !l.is_empty())
            .map(|line| {
                let mut parts = line.splitn(3, '\x1f');
                let sha = parts.next().unwrap_or("").to_string();
                let ts = parts.next().unwrap_or("").to_string();
                let msg = parts.next().unwrap_or("").to_string();
                // Truncate sha to 8 chars to mirror Python behaviour.
                let sha: String = sha.chars().take(8).collect();
                // Trim timezone seconds if desired; leave as-is to keep
                // lossless parsing for downstream consumers.
                let ts = ts.chars().take(16).collect::<String>().replace(' ', " ");
                CommitInfo {
                    sha,
                    message: msg,
                    timestamp: ts,
                }
            })
            .collect()
    }

    /// Show the diff between two commits.
    pub fn diff_commits(&self, sha1: &str, sha2: &str) -> String {
        if !self.is_initialized() {
            return String::new();
        }
        self.git_output(&["diff", sha1, sha2]).unwrap_or_default()
    }

    /// Find a commit by short-SHA prefix match.
    pub fn find_commit(&self, short_sha: &str, max_entries: usize) -> Option<CommitInfo> {
        self.log(max_entries)
            .into_iter()
            .find(|c| c.sha.starts_with(short_sha))
    }

    /// Find a commit and return it with its diff vs. the parent.
    pub fn show_commit_diff(
        &self,
        short_sha: &str,
        max_entries: usize,
    ) -> Option<(CommitInfo, String)> {
        let commits = self.log(max_entries);
        for (i, c) in commits.iter().enumerate() {
            if c.sha.starts_with(short_sha) {
                let diff = if i + 1 < commits.len() {
                    self.diff_commits(&commits[i + 1].sha, &c.sha)
                } else {
                    String::new()
                };
                return Some((c.clone(), diff));
            }
        }
        None
    }

    /// Revert (undo) the changes introduced by the given commit.
    ///
    /// Uses `git revert --no-edit` under the hood — simpler than the
    /// Python-side tree-walking and covers the same intent.
    pub fn revert(&self, commit: &str) -> Option<String> {
        if !self.is_initialized() {
            return None;
        }
        let ok = self
            .git()
            .args(["revert", "--no-edit", commit])
            .output()
            .ok()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            warn!("Git revert failed for {commit}");
            return None;
        }
        let sha = self.git_output(&["rev-parse", "--short=8", "HEAD"])?;
        Some(sha.trim().to_string())
    }

    /// Compute the age (in days) of each line in a tracked file via
    /// `git blame`.
    pub fn line_ages(&self, file_path: &str) -> Vec<LineAge> {
        if !self.is_initialized() {
            return Vec::new();
        }
        let target = self.workspace.join(file_path);
        if !target.exists() {
            return Vec::new();
        }
        if target.metadata().map(|m| m.len() == 0).unwrap_or(true) {
            return Vec::new();
        }
        let Some(out) = self.git_output(&["blame", "--date=unix", "--line-porcelain", file_path])
        else {
            return Vec::new();
        };
        let mut ages: Vec<LineAge> = Vec::new();
        let now_days = {
            use std::time::{SystemTime, UNIX_EPOCH};
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| (d.as_secs() / 86_400) as i64)
                .unwrap_or(0)
        };
        for line in out.lines() {
            if let Some(rest) = line.strip_prefix("author-time ") {
                if let Ok(ts) = rest.trim().parse::<i64>() {
                    let days = ts / 86_400;
                    ages.push(LineAge {
                        age_days: (now_days - days).max(0),
                    });
                }
            }
        }
        ages
    }
}
