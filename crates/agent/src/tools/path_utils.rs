use std::path::{Path, PathBuf};

use config::paths::get_media_dir;

const WORKSPACE_BOUNDARY_NOTE: &str = " (this is a hard policy boundary, not a transient failure; \
     do not retry with shell tricks or alternative tools, and ask \
     the user how to proceed if the resource is genuinely required)";

pub fn is_under(path: &Path, directory: &Path) -> bool {
    match directory.canonicalize() {
        Ok(resolved) => path.starts_with(&resolved),
        Err(_) => path.starts_with(directory),
    }
}

pub fn resolve_workspace_path(
    path: &str,
    workspace: Option<&Path>,
    allowed_dir: Option<&Path>,
    extra_allowed_dirs: Option<&[PathBuf]>,
) -> Result<PathBuf, String> {
    let mut p = PathBuf::from(path);
    if let Ok(rest) = p.strip_prefix("~")
        && let Some(home) = dirs::home_dir()
    {
        p = home.join(rest);
    }
    if !p.is_absolute()
        && let Some(ws) = workspace
    {
        p = ws.join(&p);
    }
    let resolved = p.canonicalize().unwrap_or_else(|_| p.clone());
    if let Some(allowed) = allowed_dir {
        let media_path = get_media_dir(None);
        let mut all_dirs = vec![allowed.to_path_buf()];
        all_dirs.push(media_path);
        if let Some(extra) = extra_allowed_dirs {
            all_dirs.extend(extra.iter().cloned());
        }
        if !all_dirs.iter().any(|d| is_under(&resolved, d)) {
            return Err(format!(
                "Path {path} is outside allowed directory {:?}{WORKSPACE_BOUNDARY_NOTE}",
                allowed
            ));
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_under_with_real_paths() {
        let tmp = std::env::temp_dir();
        let child = tmp.join("test_file.txt");
        let outside = Path::new("/tmp_nonexistent/outside.txt");
        assert!(is_under(&child, &tmp));
        assert!(!is_under(outside, &tmp));
    }

    #[test]
    fn test_resolve_relative_path() {
        let workspace = Path::new("/workspace");
        let result =
            resolve_workspace_path("subdir/file.txt", Some(workspace), None, None).unwrap();
        assert_eq!(result, Path::new("/workspace/subdir/file.txt"));
    }
}
