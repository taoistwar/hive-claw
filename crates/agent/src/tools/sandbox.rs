//! Shared path-resolution helpers and shell sandbox wrapping.
//! Port of `nanobot.agent.tools.sandbox` plus the path-policy helpers
//! shared by `filesystem`/`search`/`shell` tools in the Python source.

use std::path::{Path, PathBuf};

use thiserror::Error;

use config::get_media_dir;

#[derive(Debug, Error)]
pub enum PathError {
    #[error("Path {path} is outside allowed directory {allowed}")]
    OutsideAllowed { path: String, allowed: String },
}

/// Resolve *path* against *workspace* (if relative) and enforce the
/// restriction against *allowed_dir* and its extra peers.
///
/// `allowed_dir = None` disables the restriction entirely (legacy
/// unrestricted mode).
pub fn resolve_path(
    path: &str,
    workspace: Option<&Path>,
    allowed_dir: Option<&Path>,
    extra_allowed_dirs: &[PathBuf],
) -> Result<PathBuf, PathError> {
    let mut p = expand_user(path);
    if !p.is_absolute() {
        if let Some(ws) = workspace {
            p = ws.join(p);
        }
    }
    let resolved = canonicalize(&p);
    if let Some(allowed) = allowed_dir {
        let media = canonicalize(&get_media_dir(None));
        let mut all: Vec<PathBuf> = Vec::with_capacity(2 + extra_allowed_dirs.len());
        all.push(canonicalize(allowed));
        all.push(media);
        all.extend(extra_allowed_dirs.iter().map(|d| canonicalize(d)));
        if !all.iter().any(|d| is_under(&resolved, d)) {
            return Err(PathError::OutsideAllowed {
                path: path.to_string(),
                allowed: allowed.display().to_string(),
            });
        }
    }
    Ok(resolved)
}

/// Best-effort canonicalization. Falls back to the unresolved path so a
/// *not-yet-existing* target (e.g. for `write_file`) remains usable.
pub fn canonicalize<P: AsRef<Path>>(path: P) -> PathBuf {
    std::fs::canonicalize(&path).unwrap_or_else(|_| path.as_ref().to_path_buf())
}

/// `true` if *path* is inside *dir* (after canonicalization of *dir*).
pub fn is_under(path: &Path, dir: &Path) -> bool {
    let dir = canonicalize(dir);
    path.starts_with(&dir)
}

fn expand_user(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }
    if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    PathBuf::from(path)
}

/// Wrap *command* in a bubblewrap sandbox. The Rust port currently only
/// covers the `bwrap` backend (matching the Python default); additional
/// backends can be added by mapping in [`wrap_command`].
pub fn wrap_command(
    sandbox: &str,
    command: &str,
    workspace: &Path,
    cwd: &Path,
) -> Result<String, String> {
    match sandbox {
        "bwrap" => Ok(bwrap(command, workspace, cwd)),
        other => Err(format!(
            "Unknown sandbox backend '{other}'. Available: bwrap"
        )),
    }
}

fn bwrap(command: &str, workspace: &Path, cwd: &Path) -> String {
    let ws = canonicalize(workspace);
    let media = canonicalize(&get_media_dir(None));
    let sandbox_cwd = canonicalize(cwd);
    let sandbox_cwd = sandbox_cwd
        .strip_prefix(&ws)
        .map(|rel| ws.join(rel))
        .unwrap_or_else(|_| ws.clone());

    let mut args: Vec<String> = vec![
        "bwrap".into(),
        "--new-session".into(),
        "--die-with-parent".into(),
    ];
    let required = ["/usr"];
    let optional = [
        "/bin",
        "/lib",
        "/lib64",
        "/etc/alternatives",
        "/etc/ssl/certs",
        "/etc/resolv.conf",
        "/etc/ld.so.cache",
    ];
    for p in required {
        args.push("--ro-bind".into());
        args.push(p.into());
        args.push(p.into());
    }
    for p in optional {
        args.push("--ro-bind-try".into());
        args.push(p.into());
        args.push(p.into());
    }
    let parent = ws.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    args.extend([
        "--proc".into(),
        "/proc".into(),
        "--dev".into(),
        "/dev".into(),
        "--tmpfs".into(),
        "/tmp".into(),
        "--tmpfs".into(),
        parent.display().to_string(),
        "--dir".into(),
        ws.display().to_string(),
        "--bind".into(),
        ws.display().to_string(),
        ws.display().to_string(),
        "--ro-bind-try".into(),
        media.display().to_string(),
        media.display().to_string(),
        "--chdir".into(),
        sandbox_cwd.display().to_string(),
        "--".into(),
        "sh".into(),
        "-c".into(),
        command.to_string(),
    ]);
    shell_join(&args)
}

/// POSIX-style `shlex.join` — single-quote every argument that contains
/// characters outside `[A-Za-z0-9_@%+=:,./-]`.
pub fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|a| shell_quote(a))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    let safe = s.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '@' | '%' | '+' | '=' | ':' | ',' | '.' | '/' | '-' | '_')
    });
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_against_workspace() {
        let tmp = std::env::temp_dir();
        let ws = tmp.join("rustbot_sandbox_ws");
        let _ = std::fs::create_dir_all(&ws);
        let resolved = resolve_path("foo.txt", Some(&ws), None, &[]).unwrap();
        assert!(resolved.ends_with("foo.txt"));
    }

    #[test]
    fn rejects_outside_allowed() {
        let tmp = std::env::temp_dir();
        let ws = tmp.join("rustbot_sandbox_ws2");
        let _ = std::fs::create_dir_all(&ws);
        let res = resolve_path("/etc/passwd", Some(&ws), Some(&ws), &[]);
        assert!(matches!(res, Err(PathError::OutsideAllowed { .. })));
    }

    #[test]
    fn shell_quote_safe_passthrough() {
        assert_eq!(shell_quote("abc.txt"), "abc.txt");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }
}
