//! Search tools: `grep`. Port of `nanobot.agent.tools.search`.
//!
//! Note: `glob` tool was removed in the upstream Python codebase.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use globset::{Glob, GlobMatcher};
use regex::RegexBuilder;
use serde_json::{json, Value};
use walkdir::WalkDir;

use super::base::{Tool, ToolExecError};
use super::filesystem::FsTool;

const DEFAULT_HEAD_LIMIT: usize = 250;
const MAX_RESULT_CHARS: usize = 128_000;
const MAX_FILE_BYTES: u64 = 2_000_000;

const IGNORE_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "__pycache__",
    ".venv",
    "venv",
    "dist",
    "build",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".coverage",
    "htmlcov",
    "target",
];

fn type_globs(kind: &str) -> Vec<&'static str> {
    match kind.trim().to_ascii_lowercase().as_str() {
        "py" | "python" => vec!["*.py", "*.pyi"],
        "js" => vec!["*.js", "*.jsx", "*.mjs", "*.cjs"],
        "ts" => vec!["*.ts", "*.tsx", "*.mts", "*.cts"],
        "tsx" => vec!["*.tsx"],
        "jsx" => vec!["*.jsx"],
        "json" => vec!["*.json"],
        "md" | "markdown" => vec!["*.md", "*.mdx"],
        "go" => vec!["*.go"],
        "rs" | "rust" => vec!["*.rs"],
        "java" => vec!["*.java"],
        "sh" => vec!["*.sh", "*.bash"],
        "yaml" | "yml" => vec!["*.yaml", "*.yml"],
        "toml" => vec!["*.toml"],
        "sql" => vec!["*.sql"],
        "html" => vec!["*.html", "*.htm"],
        "css" => vec!["*.css", "*.scss", "*.sass"],
        _ => Vec::new(),
    }
}

fn matches_type(name: &str, kind: &Option<String>) -> bool {
    let Some(k) = kind.as_deref() else {
        return true;
    };
    let lower = k.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return true;
    }
    let patterns = type_globs(&lower);
    let globs: Vec<GlobMatcher> = if patterns.is_empty() {
        match Glob::new(&format!("*.{lower}")) {
            Ok(g) => vec![g.compile_matcher()],
            Err(_) => return false,
        }
    } else {
        patterns
            .iter()
            .filter_map(|p| Glob::new(p).ok().map(|g| g.compile_matcher()))
            .collect()
    };
    globs.iter().any(|g| g.is_match(name))
}

fn compile_glob(pattern: &str) -> Option<GlobMatcher> {
    Glob::new(pattern.trim()).ok().map(|g| g.compile_matcher())
}

fn is_ignored_component(comp: &str) -> bool {
    IGNORE_DIRS.iter().any(|d| *d == comp)
}

fn iter_files(root: &Path) -> impl Iterator<Item = PathBuf> {
    WalkDir::new(root)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| {
            !(e.depth() > 0
                && e.file_type().is_dir()
                && is_ignored_component(&e.file_name().to_string_lossy()))
        })
        .flatten()
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_path_buf())
}

fn display_path(target: &Path, root: &Path, workspace: Option<&Path>) -> String {
    if let Some(ws) = workspace {
        if let Ok(rel) = target.strip_prefix(ws) {
            return rel.to_string_lossy().replace('\\', "/");
        }
    }
    target
        .strip_prefix(root)
        .unwrap_or(target)
        .to_string_lossy()
        .replace('\\', "/")
}

fn file_mtime_secs(path: &Path) -> f64 {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn is_binary(raw: &[u8]) -> bool {
    if raw.contains(&0u8) {
        return true;
    }
    let sample = &raw[..raw.len().min(4096)];
    if sample.is_empty() {
        return false;
    }
    let non_text = sample
        .iter()
        .filter(|&&b| b < 9 || (b > 13 && b < 32))
        .count();
    (non_text as f64 / sample.len() as f64) > 0.2
}

fn paginate<T: Clone>(items: &[T], limit: Option<usize>, offset: usize) -> (Vec<T>, bool) {
    match limit {
        None => (items.iter().skip(offset).cloned().collect(), false),
        Some(l) => {
            let end = offset + l;
            let sliced: Vec<T> = items.iter().skip(offset).take(l).cloned().collect();
            let truncated = items.len() > end;
            (sliced, truncated)
        }
    }
}

fn pagination_note(limit: Option<usize>, offset: usize, truncated: bool) -> Option<String> {
    if truncated {
        return match limit {
            None => Some(format!("(pagination: offset={offset})")),
            Some(l) => Some(format!("(pagination: limit={l}, offset={offset})")),
        };
    }
    if offset > 0 {
        return Some(format!("(pagination: offset={offset})"));
    }
    None
}

// ---------------------------------------------------------------------------
// grep
// ---------------------------------------------------------------------------

pub struct GrepTool(pub FsTool);

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }
    fn description(&self) -> String {
        "Search file contents with a regex pattern. Default output_mode is files_with_matches; use 'content' for matching lines with context. Skips binary and files >2 MB.".into()
    }
    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{
                "pattern":{"type":"string","description":"Regex or plain text pattern","minLength":1},
                "path":{"type":"string","description":"File or directory to search in (default '.')"},
                "glob":{"type":"string","description":"Optional file filter, e.g. '*.py'"},
                "type":{"type":"string","description":"File type shorthand: 'py','ts','md','json',..."},
                "case_insensitive":{"type":"boolean","default":false},
                "fixed_strings":{"type":"boolean","default":false},
                "output_mode":{"type":"string","enum":["content","files_with_matches","count"]},
                "context_before":{"type":"integer","minimum":0,"maximum":20},
                "context_after":{"type":"integer","minimum":0,"maximum":20},
                "head_limit":{"type":"integer","minimum":0,"maximum":1000},
                "offset":{"type":"integer","minimum":0},
            },
            "required":["pattern"],
        })
    }
    fn read_only(&self) -> bool {
        true
    }
    fn scopes(&self) -> &[&str] {
        &["core", "subagent"]
    }
    async fn execute(&self, params: Value) -> Result<Value, ToolExecError> {
        let Some(pattern) = params.get("pattern").and_then(|v| v.as_str()) else {
            return Ok(Value::String("Error: pattern required".into()));
        };
        let path = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let glob_pat = params.get("glob").and_then(|v| v.as_str()).map(String::from);
        let type_kind = params.get("type").and_then(|v| v.as_str()).map(String::from);
        let case_insensitive = params
            .get("case_insensitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let fixed_strings = params
            .get("fixed_strings")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let output_mode = params
            .get("output_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("files_with_matches")
            .to_string();
        let context_before = params
            .get("context_before")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let context_after = params
            .get("context_after")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let head_limit = params.get("head_limit").and_then(|v| v.as_u64());
        let offset = params
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let limit: Option<usize> = match head_limit {
            Some(0) => None,
            Some(n) => Some(n as usize),
            None => Some(DEFAULT_HEAD_LIMIT),
        };

        let target = match self.0.resolve(path) {
            Ok(p) => p,
            Err(e) => return Ok(Value::String(format!("Error: {e}"))),
        };
        if !target.exists() {
            return Ok(Value::String(format!("Error: Path not found: {path}")));
        }
        let needle = if fixed_strings {
            regex::escape(pattern)
        } else {
            pattern.to_string()
        };
        let regex = match RegexBuilder::new(&needle)
            .case_insensitive(case_insensitive)
            .build()
        {
            Ok(r) => r,
            Err(e) => return Ok(Value::String(format!("Error: invalid regex pattern: {e}"))),
        };
        let glob_matcher = glob_pat.as_deref().and_then(compile_glob);
        let root = if target.is_dir() {
            target.clone()
        } else {
            target.parent().unwrap_or(&target).to_path_buf()
        };

        let mut blocks: Vec<String> = Vec::new();
        let mut result_chars = 0;
        let mut seen_content_matches = 0usize;
        let mut truncated = false;
        let mut size_truncated = false;
        let mut skipped_binary = 0usize;
        let mut skipped_large = 0usize;
        let mut matching_files: Vec<String> = Vec::new();
        let mut counts: HashMap<String, usize> = HashMap::new();
        let mut file_mtimes: HashMap<String, f64> = HashMap::new();

        let files: Vec<PathBuf> = if target.is_file() {
            vec![target.clone()]
        } else {
            iter_files(&target).collect()
        };

        'outer: for file_path in files {
            let name = file_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            let rel_path = file_path
                .strip_prefix(&root)
                .unwrap_or(&file_path)
                .to_string_lossy()
                .replace('\\', "/");
            if let Some(gm) = &glob_matcher {
                if !(gm.is_match(&rel_path) || gm.is_match(name)) {
                    continue;
                }
            }
            if !matches_type(name, &type_kind) {
                continue;
            }
            let size = fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0);
            if size > MAX_FILE_BYTES {
                skipped_large += 1;
                continue;
            }
            let raw = match fs::read(&file_path) {
                Ok(r) => r,
                Err(_) => {
                    skipped_binary += 1;
                    continue;
                }
            };
            if is_binary(&raw) {
                skipped_binary += 1;
                continue;
            }
            let content = match String::from_utf8(raw) {
                Ok(c) => c,
                Err(_) => {
                    skipped_binary += 1;
                    continue;
                }
            };
            let lines: Vec<&str> = content.split('\n').collect();
            let mtime = file_mtime_secs(&file_path);
            let display = display_path(&file_path, &root, self.0.workspace.as_deref());
            let mut file_had_match = false;
            for (idx0, line) in lines.iter().enumerate() {
                if !regex.is_match(line) {
                    continue;
                }
                let line_no = idx0 + 1;
                file_had_match = true;

                match output_mode.as_str() {
                    "count" => {
                        *counts.entry(display.clone()).or_insert(0) += 1;
                    }
                    "files_with_matches" => {
                        if !matching_files.iter().any(|f| f == &display) {
                            matching_files.push(display.clone());
                            file_mtimes.insert(display.clone(), mtime);
                        }
                        break;
                    }
                    _ => {
                        seen_content_matches += 1;
                        if seen_content_matches <= offset {
                            continue;
                        }
                        if let Some(l) = limit {
                            if blocks.len() >= l {
                                truncated = true;
                                break;
                            }
                        }
                        let block = format_block(
                            &display,
                            &lines,
                            line_no,
                            context_before,
                            context_after,
                        );
                        let extra_sep = if blocks.is_empty() { 0 } else { 2 };
                        if result_chars + extra_sep + block.len() > MAX_RESULT_CHARS {
                            size_truncated = true;
                            break;
                        }
                        result_chars += extra_sep + block.len();
                        blocks.push(block);
                    }
                }
            }
            if output_mode == "count" && file_had_match {
                if !matching_files.iter().any(|f| f == &display) {
                    matching_files.push(display.clone());
                    file_mtimes.insert(display.clone(), mtime);
                }
            }
            if truncated || size_truncated {
                break 'outer;
            }
        }

        let mut result = match output_mode.as_str() {
            "files_with_matches" => {
                if matching_files.is_empty() {
                    format!("No matches found for pattern '{pattern}' in {path}")
                } else {
                    let mut ordered = matching_files.clone();
                    ordered.sort_by(|a, b| {
                        let am = file_mtimes.get(a).copied().unwrap_or(0.0);
                        let bm = file_mtimes.get(b).copied().unwrap_or(0.0);
                        bm.partial_cmp(&am)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then(a.cmp(b))
                    });
                    let (paged, trunc) = paginate(&ordered, limit, offset);
                    truncated = trunc;
                    paged.join("\n")
                }
            }
            "count" => {
                if counts.is_empty() {
                    format!("No matches found for pattern '{pattern}' in {path}")
                } else {
                    let mut ordered = matching_files.clone();
                    ordered.sort_by(|a, b| {
                        let am = file_mtimes.get(a).copied().unwrap_or(0.0);
                        let bm = file_mtimes.get(b).copied().unwrap_or(0.0);
                        bm.partial_cmp(&am)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then(a.cmp(b))
                    });
                    let (paged, trunc) = paginate(&ordered, limit, offset);
                    truncated = trunc;
                    paged
                        .into_iter()
                        .map(|n| {
                            let c = counts.get(&n).copied().unwrap_or(0);
                            format!("{n}: {c}")
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            }
            _ => {
                if blocks.is_empty() {
                    format!("No matches found for pattern '{pattern}' in {path}")
                } else {
                    blocks.join("\n\n")
                }
            }
        };

        let mut notes: Vec<String> = Vec::new();
        if output_mode == "content" && truncated {
            notes.push(format!(
                "(pagination: limit={}, offset={offset})",
                limit.map(|l| l.to_string()).unwrap_or_else(|| "∞".into())
            ));
        } else if output_mode == "content" && size_truncated {
            notes.push("(output truncated due to size)".into());
        } else if truncated {
            notes.push(format!(
                "(pagination: limit={}, offset={offset})",
                limit.map(|l| l.to_string()).unwrap_or_else(|| "∞".into())
            ));
        } else if offset > 0 {
            notes.push(format!("(pagination: offset={offset})"));
        }
        if skipped_binary > 0 {
            notes.push(format!("(skipped {skipped_binary} binary/unreadable files)"));
        }
        if skipped_large > 0 {
            notes.push(format!("(skipped {skipped_large} large files)"));
        }
        if output_mode == "count" && !counts.is_empty() {
            let total: usize = counts.values().sum();
            notes.push(format!(
                "(total matches: {total} in {} files)",
                counts.len()
            ));
        }
        if !notes.is_empty() {
            result.push_str("\n\n");
            result.push_str(&notes.join("\n"));
        }
        Ok(Value::String(result))
    }
}

fn format_block(
    display: &str,
    lines: &[&str],
    match_line: usize,
    before: usize,
    after: usize,
) -> String {
    let start = match_line.saturating_sub(before).max(1);
    let end = (match_line + after).min(lines.len());
    let mut block = vec![format!("{display}:{match_line}")];
    for ln in start..=end {
        let marker = if ln == match_line { ">" } else { " " };
        block.push(format!("{marker} {ln}| {}", lines[ln - 1]));
    }
    block.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    fn setup(tag: &str) -> PathBuf {
        let ws = temp_dir().join(format!("rustbot_search_ws_{tag}"));
        let _ = fs::remove_dir_all(&ws);
        fs::create_dir_all(&ws).unwrap();
        fs::write(ws.join("a.rs"), "fn hello() { println!(\"hi\"); }").unwrap();
        fs::write(ws.join("b.md"), "# Title\nhello world\n").unwrap();
        fs::create_dir_all(ws.join("sub")).unwrap();
        fs::write(ws.join("sub/c.rs"), "fn hello2() {}\n").unwrap();
        ws
    }

    fn fs_tool(ws: &Path) -> FsTool {
        FsTool::new(Some(ws.to_path_buf()), Some(ws.to_path_buf()), Vec::new())
    }

    #[tokio::test]
    async fn grep_files_with_matches() {
        let ws = setup("grep");
        let tool = GrepTool(fs_tool(&ws));
        let out = tool
            .execute(json!({"pattern":"hello"}))
            .await
            .unwrap();
        let out = out.as_str().unwrap();
        assert!(out.contains("a.rs"));
        assert!(out.contains("b.md"));
    }
}
