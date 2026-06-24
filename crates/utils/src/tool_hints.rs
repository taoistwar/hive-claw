//! Tool-hint formatting for concise, human-readable tool-call display
//! (port of `nanobot.utils.tool_hints`).

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

use crate::path::{abbreviate_path, abbreviate_path_with_len};

/// A minimal shape describing a tool call. Callers that already have a
/// richer `ToolCall` type should implement `From<ToolCall> for ToolCallView`
/// or construct this view directly.
#[derive(Debug, Clone)]
pub struct ToolCallView {
    pub name: String,
    /// `arguments` is a JSON value so we can model both list + dict shapes
    /// coming from the Python original.
    pub arguments: Value,
}

#[derive(Debug, Clone, Copy)]
struct ToolFormat {
    key_args: &'static [&'static str],
    template: &'static str,
    is_path: bool,
    is_command: bool,
}

fn tool_format(name: &str) -> Option<ToolFormat> {
    match name {
        "read_file" => Some(ToolFormat {
            key_args: &["path", "file_path"],
            template: "read {}",
            is_path: true,
            is_command: false,
        }),
        "write_file" => Some(ToolFormat {
            key_args: &["path", "file_path"],
            template: "write {}",
            is_path: true,
            is_command: false,
        }),
        "edit" => Some(ToolFormat {
            key_args: &["file_path", "path"],
            template: "edit {}",
            is_path: true,
            is_command: false,
        }),
        "glob" => Some(ToolFormat {
            key_args: &["pattern"],
            template: r#"glob "{}""#,
            is_path: false,
            is_command: false,
        }),
        "grep" => Some(ToolFormat {
            key_args: &["pattern"],
            template: r#"grep "{}""#,
            is_path: false,
            is_command: false,
        }),
        "exec" => Some(ToolFormat {
            key_args: &["command"],
            template: "$ {}",
            is_path: false,
            is_command: true,
        }),
        "web_search" => Some(ToolFormat {
            key_args: &["query"],
            template: r#"search "{}""#,
            is_path: false,
            is_command: false,
        }),
        "web_fetch" => Some(ToolFormat {
            key_args: &["url"],
            template: "fetch {}",
            is_path: true,
            is_command: false,
        }),
        "list_dir" => Some(ToolFormat {
            key_args: &["path"],
            template: "ls {}",
            is_path: true,
            is_command: false,
        }),
        _ => None,
    }
}

// Matches file paths embedded in shell commands, including quoted paths with
// spaces. Rust's `regex` crate does not support backreferences or the `(?<=)`
// look-behind used in the Python original; we emulate the "bare path must be
// preceded by whitespace when starting with `/`" rule by alternating
// start-of-string with a whitespace-prefixed variant.
static PATH_IN_CMD_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#""(?P<double>(?:[A-Za-z]:[/\\]|~/|/)[^"]+)"|'(?P<single>(?:[A-Za-z]:[/\\]|~/|/)[^']+)'|(?P<bare>(?:[A-Za-z]:[/\\]|~/|/)[^\s;&|<>"']+)"#,
    )
    .unwrap()
});

/// Format tool calls as concise hints with smart abbreviation.
pub fn format_tool_hints(tool_calls: &[ToolCallView]) -> String {
    if tool_calls.is_empty() {
        return String::new();
    }
    let formatted: Vec<String> = tool_calls
        .iter()
        .map(|tc| {
            if let Some(fmt) = tool_format(&tc.name) {
                fmt_known(tc, fmt)
            } else if tc.name.starts_with("mcp_") {
                fmt_mcp(tc)
            } else {
                fmt_fallback(tc)
            }
        })
        .collect();

    // Collapse repeats.
    let mut hints: Vec<(String, u32)> = Vec::new();
    for hint in formatted {
        if let Some(last) = hints.last_mut() {
            if last.0 == hint {
                last.1 += 1;
                continue;
            }
        }
        hints.push((hint, 1));
    }
    hints
        .into_iter()
        .map(|(h, c)| if c > 1 { format!("{h} \u{00d7} {c}") } else { h })
        .collect::<Vec<_>>()
        .join(", ")
}

fn get_args(tc: &ToolCallView) -> &Value {
    match &tc.arguments {
        Value::Array(arr) => arr.first().unwrap_or(&Value::Null),
        other => other,
    }
}

fn extract_arg(tc: &ToolCallView, key_args: &[&str]) -> Option<String> {
    let args = get_args(tc);
    let obj = args.as_object()?;
    for key in key_args {
        if let Some(v) = obj.get(*key).and_then(Value::as_str) {
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    for v in obj.values() {
        if let Some(s) = v.as_str() {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn fmt_known(tc: &ToolCallView, fmt: ToolFormat) -> String {
    let Some(mut val) = extract_arg(tc, fmt.key_args) else {
        return tc.name.clone();
    };
    if fmt.is_path {
        val = abbreviate_path(&val);
    } else if fmt.is_command {
        val = abbreviate_command(&val, 40);
    }
    fmt.template.replacen("{}", &val, 1)
}

fn abbreviate_command(cmd: &str, max_len: usize) -> String {
    let replaced = PATH_IN_CMD_RE.replace_all(cmd, |caps: &regex::Captures| {
        if let Some(m) = caps.name("double") {
            format!("\"{}\"", abbreviate_path_with_len(m.as_str(), 25))
        } else if let Some(m) = caps.name("single") {
            format!("'{}'", abbreviate_path_with_len(m.as_str(), 25))
        } else if let Some(m) = caps.name("bare") {
            abbreviate_path_with_len(m.as_str(), 25)
        } else {
            caps.get(0).unwrap().as_str().to_string()
        }
    });
    let replaced = replaced.into_owned();
    if replaced.chars().count() <= max_len {
        return replaced;
    }
    let take = max_len.saturating_sub(1);
    let head: String = replaced.chars().take(take).collect();
    format!("{head}\u{2026}")
}

fn fmt_mcp(tc: &ToolCallView) -> String {
    let name = &tc.name;
    let (server, tool) = if let Some(idx) = name.find("__") {
        let server = name[..idx].trim_start_matches("mcp_").to_string();
        let tool = name[idx + 2..].to_string();
        (server, tool)
    } else {
        let rest = name.strip_prefix("mcp_").unwrap_or(name.as_str());
        match rest.split_once('_') {
            Some((a, b)) => (a.to_string(), b.to_string()),
            None => (rest.to_string(), String::new()),
        }
    };
    if tool.is_empty() {
        return name.clone();
    }
    let args = get_args(tc);
    let first_str = args
        .as_object()
        .and_then(|o| o.values().find_map(|v| v.as_str().filter(|s| !s.is_empty())));
    match first_str {
        Some(v) => format!("{server}::{tool}(\"{}\")", abbreviate_path_with_len(v, 40)),
        None => format!("{server}::{tool}"),
    }
}

fn fmt_fallback(tc: &ToolCallView) -> String {
    let args = get_args(tc);
    let val = args
        .as_object()
        .and_then(|o| o.values().next().and_then(Value::as_str));
    let Some(v) = val else {
        return tc.name.clone();
    };
    if v.chars().count() > 40 {
        format!("{}(\"{}\")", tc.name, abbreviate_path_with_len(v, 40))
    } else {
        format!("{}(\"{}\")", tc.name, v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tc(name: &str, args: Value) -> ToolCallView {
        ToolCallView {
            name: name.into(),
            arguments: args,
        }
    }

    #[test]
    fn known_read_file() {
        let hint = format_tool_hints(&[tc(
            "read_file",
            json!({ "path": "/home/u/project/main.rs" }),
        )]);
        assert!(hint.starts_with("read "), "got {hint}");
        assert!(hint.ends_with("main.rs"), "got {hint}");
    }

    #[test]
    fn glob_quoted() {
        let hint = format_tool_hints(&[tc("glob", json!({ "pattern": "*.py" }))]);
        assert_eq!(hint, "glob \"*.py\"");
    }

    #[test]
    fn mcp_tool_with_server_split() {
        let hint = format_tool_hints(&[tc(
            "mcp_github__get_issue",
            json!({ "owner": "foo" }),
        )]);
        assert!(hint.starts_with("github::get_issue"), "got {hint}");
    }

    #[test]
    fn repeats_collapsed() {
        let calls = vec![
            tc("glob", json!({"pattern": "*.py"})),
            tc("glob", json!({"pattern": "*.py"})),
        ];
        let hint = format_tool_hints(&calls);
        assert!(hint.ends_with("\u{00d7} 2"), "got {hint}");
    }
}
