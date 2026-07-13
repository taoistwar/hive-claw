//! Path abbreviation utilities for display (Rust port of
//! `nanobot.utils.path`).

use once_cell::sync::Lazy;
use regex::Regex;

static HTTP_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^https?://").unwrap());

/// Abbreviate a file path or URL, preserving basename and key directories.
///
/// Strategy:
/// 1. Return as-is if short enough.
/// 2. Replace home directory with `~/`.
/// 3. From the right, keep basename + parent dirs until the budget is
///    exhausted.
/// 4. Prefix with `…/`.
pub fn abbreviate_path(path: &str) -> String {
    abbreviate_path_with_len(path, 40)
}

pub fn abbreviate_path_with_len(path: &str, max_len: usize) -> String {
    if path.is_empty() {
        return path.to_string();
    }

    // URL shortcut.
    if HTTP_RE.is_match(path) {
        return abbreviate_url(path, max_len);
    }

    // Normalize separators to `/`.
    let mut normalized = path.replace('\\', "/");

    // Replace home directory.
    if let Some(home) = dirs::home_dir() {
        let home = home.to_string_lossy().replace('\\', "/");
        if normalized == home {
            normalized = "~".into();
        } else if let Some(rest) = normalized.strip_prefix(&(home.clone() + "/")) {
            normalized = format!("~/{rest}");
        }
    }

    if normalized.chars().count() <= max_len {
        return normalized;
    }

    let trimmed = normalized.trim_end_matches('/').to_string();
    let parts: Vec<&str> = trimmed.split('/').collect();
    if parts.len() <= 1 {
        return truncate_with_ellipsis(&normalized, max_len);
    }

    let basename = parts.last().copied().unwrap_or("");
    // Budget: max_len − 3 chars for `…/` prefix + final `/`.
    let mut budget: isize = max_len as isize - basename.chars().count() as isize - 3;

    let mut kept: Vec<&str> = Vec::new();
    for seg in parts[..parts.len() - 1].iter().rev() {
        let needed = seg.chars().count() as isize + 1;
        if kept.is_empty() && needed <= budget {
            kept.push(seg);
            budget -= needed;
        } else if !kept.is_empty() {
            if needed <= budget {
                kept.push(seg);
                budget -= needed;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    kept.reverse();
    if kept.is_empty() {
        format!("\u{2026}/{basename}")
    } else {
        format!("\u{2026}/{}/{}", kept.join("/"), basename)
    }
}

fn truncate_with_ellipsis(s: &str, max_len: usize) -> String {
    if max_len == 0 {
        return String::new();
    }
    let take = max_len - 1;
    let truncated: String = s.chars().take(take).collect();
    format!("{truncated}\u{2026}")
}

/// Very small URL parser that only cares about host + path — we do not need
/// query / fragment handling here. Returns (netloc, path).
fn split_url(url: &str) -> Option<(&str, &str)> {
    let idx = url.find("://")?;
    let after = &url[idx + 3..];
    let cut = after.find('/').unwrap_or(after.len());
    let (netloc, rest) = after.split_at(cut);
    // rest starts with `/` or is empty. Also strip query / fragment.
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    Some((netloc, rest))
}

fn abbreviate_url(url: &str, max_len: usize) -> String {
    if url.chars().count() <= max_len {
        return url.to_string();
    }

    let Some((domain, path_part)) = split_url(url) else {
        return url.to_string();
    };
    let segments: Vec<&str> = path_part.trim_end_matches('/').split('/').collect();
    let basename = segments.last().copied().unwrap_or("");
    if basename.is_empty() {
        return truncate_with_ellipsis(url, max_len);
    }

    let budget_signed =
        max_len as isize - domain.chars().count() as isize - basename.chars().count() as isize - 4;
    if budget_signed < 0 {
        let trunc = max_len as isize - domain.chars().count() as isize - 5;
        let taken: String = if trunc > 0 {
            basename.chars().take(trunc as usize).collect()
        } else {
            String::new()
        };
        return format!("{domain}/\u{2026}/{taken}");
    }
    let mut budget = budget_signed;

    let mut kept: Vec<&str> = Vec::new();
    // Walk backwards from parent (segments except basename).
    let parent_iter = if segments.len() > 1 {
        segments[..segments.len() - 1].iter().rev()
    } else {
        [].iter().rev()
    };
    for seg in parent_iter {
        let needed = seg.chars().count() as isize + 1;
        if needed <= budget {
            kept.push(seg);
            budget -= needed;
        } else {
            break;
        }
    }

    kept.reverse();
    if kept.is_empty() {
        format!("{domain}/\u{2026}/{basename}")
    } else {
        format!("{domain}/\u{2026}/{}/{basename}", kept.join("/"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_returns_unchanged() {
        assert_eq!(abbreviate_path("/a/b"), "/a/b");
    }

    #[test]
    fn long_path_collapses_with_ellipsis() {
        let p = "/var/lib/some/really/deeply/nested/directory/file.txt";
        let got = abbreviate_path(p);
        assert!(got.ends_with("file.txt"), "got = {got}");
        assert!(got.starts_with('\u{2026}'), "got = {got}");
        assert!(
            got.chars().count() <= 40,
            "got len = {}",
            got.chars().count()
        );
    }

    #[test]
    fn url_keeps_domain_and_filename() {
        let url = "https://raw.githubusercontent.com/owner/repo/refs/heads/main/path/to/file.json";
        let got = abbreviate_path(url);
        assert!(got.contains("raw.githubusercontent.com"), "got = {got}");
        assert!(got.ends_with("file.json"), "got = {got}");
    }
}
