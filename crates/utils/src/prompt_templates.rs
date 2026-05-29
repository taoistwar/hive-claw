//! Minimal prompt-template renderer (port of
//! `nanobot.utils.prompt_templates`).
//!
//! The Python original uses Jinja2 against `templates/` under the installed
//! package. This module provides a lightweight subset of Jinja2 features:
//!
//! * `{{ key }}` variable substitution
//! * `{% include "filename" %}` — reads and includes another template file
//! * `{% if condition %}...{% elif condition %}...{% else %}...{% endif %}`
//!   blocks with support for `==`, `!=`, `or`, `and` in conditions
//! * `{% raw %}...{% endraw %}` — outputs content verbatim without processing
//!
//! For callers that need full Jinja2 compatibility (loops, macros, inheritance),
//! swap in a `minijinja`-backed implementation in their own crate.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Source of a template (typically a file on disk).
pub trait TemplateLoader: Send + Sync {
    fn load(&self, name: &str) -> Option<String>;
}

/// A [`TemplateLoader`] that reads files relative to a root directory
/// (mirrors Python's `FileSystemLoader`).
#[derive(Debug, Clone)]
pub struct FileSystemLoader {
    root: PathBuf,
}

impl FileSystemLoader {
    pub fn new<P: AsRef<Path>>(root: P) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }
}

impl TemplateLoader for FileSystemLoader {
    fn load(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(name)).ok()
    }
}

#[derive(Debug, PartialEq, Clone)]
enum Token {
    Text(String),
    Variable(String),
    Include(String),
    If(String),
    Elif(String),
    Else,
    Endif,
    Raw,
    Endraw,
}

fn tokenize(src: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = src.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if i + 1 < len && chars[i] == '{' && chars[i + 1] == '%' {
            let mut j = i + 2;
            while j + 1 < len && !(chars[j] == '%' && chars[j + 1] == '}') {
                j += 1;
            }
            let tag_content: String = chars[i + 2..j].iter().collect();
            let trimmed = tag_content.trim();
            let end_pos = if j + 2 < len { j + 2 } else { len };
            i = end_pos;

            let lower = trimmed.to_lowercase();
            if lower.starts_with("include ") {
                let name = trimmed[8..].trim();
                let name = name.strip_prefix('"').and_then(|s| s.strip_suffix('"'))
                    .or_else(|| name.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
                    .unwrap_or(name);
                tokens.push(Token::Include(name.to_string()));
            } else if let Some(_condition) = lower.strip_prefix("if ") {
                let original_condition = trimmed[3..].trim();
                tokens.push(Token::If(original_condition.to_string()));
            } else if let Some(_condition) = lower.strip_prefix("elif ") {
                let original_condition = trimmed[5..].trim();
                tokens.push(Token::Elif(original_condition.to_string()));
            } else if lower == "elif" {
                tokens.push(Token::Elif(String::new()));
            } else if lower == "else" {
                tokens.push(Token::Else);
            } else if lower == "endif" {
                tokens.push(Token::Endif);
            } else if lower == "raw" {
                tokens.push(Token::Raw);
            } else if lower == "endraw" {
                tokens.push(Token::Endraw);
            } else {
                tokens.push(Token::Text(format!("{{% {} %}}", tag_content)));
            }
        } else if i + 1 < len && chars[i] == '{' && chars[i + 1] == '{' {
            let mut j = i + 2;
            while j + 1 < len && !(chars[j] == '}' && chars[j + 1] == '}') {
                j += 1;
            }
            let var_name: String = chars[i + 2..j].iter().collect();
            let end_pos = if j + 2 < len { j + 2 } else { len };
            i = end_pos;
            tokens.push(Token::Variable(var_name.trim().to_string()));
        } else {
            let mut text = String::new();
            while i < len {
                if i + 1 < len && ((chars[i] == '{' && chars[i + 1] == '%') || (chars[i] == '{' && chars[i + 1] == '{')) {
                    break;
                }
                text.push(chars[i]);
                i += 1;
            }
            if !text.is_empty() {
                tokens.push(Token::Text(text));
            }
        }
    }

    tokens
}

fn render_tokens(
    tokens: &[Token],
    loader: &dyn TemplateLoader,
    context: &HashMap<String, String>,
    strip: bool,
    include_depth: usize,
) -> String {
    if include_depth > 10 {
        return tokens.iter().filter_map(|t| match t {
            Token::Text(s) => Some(s.clone()),
            Token::Variable(name) => context.get(name).cloned(),
            _ => None,
        }).collect();
    }

    let mut result = String::new();
    let mut i = 0;

    while i < tokens.len() {
        match &tokens[i] {
            Token::Text(s) => {
                result.push_str(s);
            }
            Token::Variable(name) => {
                if let Some(value) = context.get(name) {
                    result.push_str(value);
                } else {
                    result.push_str(&format!("{{{{ {} }}}}", name));
                }
            }
            Token::Include(name) => {
                if let Some(src) = loader.load(name) {
                    let included_tokens = tokenize(&src);
                    result.push_str(&render_tokens(&included_tokens, loader, context, false, include_depth + 1));
                }
            }
            Token::If(condition) => {
                let (branches_consumed, rendered) = process_if_chain(&tokens[i..], condition.clone(), loader, context, strip, include_depth + 1);
                result.push_str(&rendered);
                i += branches_consumed;
                continue;
            }
            Token::Raw => {
                let mut raw_content = String::new();
                i += 1;
                while i < tokens.len() {
                    if matches!(tokens[i], Token::Endraw) {
                        i += 1;
                        break;
                    }
                    if let Token::Text(s) = &tokens[i] {
                        raw_content.push_str(s);
                    } else {
                        match &tokens[i] {
                            Token::Variable(name) => raw_content.push_str(&format!("{{{{ {} }}}}", name)),
                            Token::Include(name) => raw_content.push_str(&format!("{{% include '{}' %}}", name)),
                            Token::If(cond) => raw_content.push_str(&format!("{{% if {} %}}", cond)),
                            Token::Elif(cond) => raw_content.push_str(&format!("{{% elif {} %}}", cond)),
                            Token::Else => raw_content.push_str("{% else %}"),
                            Token::Endif => raw_content.push_str("{% endif %}"),
                            Token::Raw => raw_content.push_str("{% raw %}"),
                            Token::Endraw => raw_content.push_str("{% endraw %}"),
                            _ => {}
                        }
                    }
                    i += 1;
                }
                result.push_str(&raw_content);
                continue;
            }
            Token::Elif(_) | Token::Else | Token::Endif | Token::Endraw => {
            }
        }
        i += 1;
    }

    if strip {
        result = result.trim_end().to_string();
    }
    result
}

fn process_if_chain(
    tokens: &[Token],
    first_condition: String,
    loader: &dyn TemplateLoader,
    context: &HashMap<String, String>,
    strip: bool,
    include_depth: usize,
) -> (usize, String) {
    let branches = extract_if_branches(tokens);

    for (i, (cond, body)) in branches.iter().enumerate() {
        let actual_cond = if i == 0 {
            &first_condition
        } else {
            match cond {
                Some(c) => c,
                None => "",
            }
        };

        let should_render = if i == 0 {
            evaluate_condition(actual_cond, context)
        } else if cond.is_some() {
            evaluate_condition(actual_cond, context)
        } else {
            true
        };

        if should_render {
            let rendered = render_tokens(body, loader, context, strip, include_depth);
            let total_consumed = 1 + calculate_branches_consumed(&branches);
            return (total_consumed, rendered);
        }
    }

    let total_consumed = 1 + calculate_branches_consumed(&branches);
    (total_consumed, String::new())
}

fn calculate_branches_consumed(branches: &[(Option<String>, Vec<Token>)]) -> usize {
    let mut count = 0;
    for (i, (_cond, body)) in branches.iter().enumerate() {
        count += 1;
        count += body.len();
        if i < branches.len() - 1 {
            count += 1;
        }
    }
    count
}

fn extract_if_branches(tokens: &[Token]) -> Vec<(Option<String>, Vec<Token>)> {
    let mut branches: Vec<(Option<String>, Vec<Token>)> = Vec::new();
    let mut current_tokens: Vec<Token> = Vec::new();
    let mut depth = 1;
    let mut i = 1;
    let mut pending_cond: Option<String> = None;

    while i < tokens.len() {
        match &tokens[i] {
            Token::If(_cond) => {
                if depth > 0 {
                    current_tokens.push(tokens[i].clone());
                }
                depth += 1;
            }
            Token::Endif => {
                if depth == 1 {
                    if pending_cond.is_some() {
                        branches.push((pending_cond.take(), current_tokens));
                    } else {
                        branches.push((None, current_tokens));
                    }
                    return branches;
                }
                depth -= 1;
                if depth > 0 {
                    current_tokens.push(tokens[i].clone());
                }
            }
            Token::Elif(cond) if depth == 1 => {
                branches.push((pending_cond.take(), current_tokens));
                current_tokens = Vec::new();
                pending_cond = Some(cond.clone());
            }
            Token::Else if depth == 1 => {
                branches.push((pending_cond.take(), current_tokens));
                current_tokens = Vec::new();
            }
            _ => {
                current_tokens.push(tokens[i].clone());
            }
        }
        i += 1;
    }

    branches.push((pending_cond.take(), current_tokens));
    branches
}

fn evaluate_condition(condition: &str, context: &HashMap<String, String>) -> bool {
    let trimmed = condition.trim();
    let or_parts = split_or(trimmed);
    for part in or_parts {
        if evaluate_and_expr(part.trim(), context) {
            return true;
        }
    }
    false
}

fn evaluate_and_expr(expr: &str, context: &HashMap<String, String>) -> bool {
    let and_parts = split_and(expr);
    for part in and_parts {
        if !evaluate_single_condition(part.trim(), context) {
            return false;
        }
    }
    true
}

fn evaluate_single_condition(condition: &str, context: &HashMap<String, String>) -> bool {
    let trimmed = condition.trim();

    if let Some(pos) = find_comparison_op(trimmed, "==") {
        let left = trimmed[..pos].trim();
        let right = trimmed[pos + 2..].trim();
        return resolve_value(left, context) == resolve_value(right, context);
    }

    if let Some(pos) = find_comparison_op(trimmed, "!=") {
        let left = trimmed[..pos].trim();
        let right = trimmed[pos + 2..].trim();
        return resolve_value(left, context) != resolve_value(right, context);
    }

    let val = resolve_value(trimmed, context);
    !val.is_empty() && val != "false" && val != "0"
}

fn find_comparison_op(s: &str, op: &str) -> Option<usize> {
    let mut in_string = false;
    let mut string_char = '\0';
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();

    for i in 0..len {
        if in_string {
            if chars[i] == string_char {
                in_string = false;
            }
            continue;
        }
        if chars[i] == '"' || chars[i] == '\'' {
            in_string = true;
            string_char = chars[i];
            continue;
        }
        if i + op.len() <= len && &s[i..i + op.len()] == op {
            return Some(i);
        }
    }
    None
}

fn resolve_value(token: &str, context: &HashMap<String, String>) -> String {
    let trimmed = token.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
    {
        return trimmed[1..trimmed.len() - 1].to_string();
    }
    context.get(trimmed).cloned().unwrap_or_default()
}

fn split_or(s: &str) -> Vec<&str> {
    split_logical(s, " or ")
}

fn split_and(s: &str) -> Vec<&str> {
    split_logical(s, " and ")
}

fn split_logical<'a>(s: &'a str, op: &str) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    let mut in_string = false;
    let mut string_char = '\0';
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let op_len = op.len();
    let mut i = 0;

    while i < len {
        if in_string {
            if chars[i] == string_char {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if chars[i] == '"' || chars[i] == '\'' {
            in_string = true;
            string_char = chars[i];
            i += 1;
            continue;
        }
        if chars[i] == '(' {
            depth += 1;
        } else if chars[i] == ')' {
            depth -= 1;
        } else if depth == 0 && i + op_len <= len {
            if &s[i..i + op_len] == op {
                parts.push(&s[start..i]);
                start = i + op_len;
                i += op_len;
                continue;
            }
        }
        i += 1;
    }

    parts.push(&s[start..]);
    parts
}

/// Render a template with variable substitution, include, and if-block support.
pub fn render_template(
    loader: &dyn TemplateLoader,
    name: &str,
    context: &HashMap<String, String>,
    strip: bool,
) -> Option<String> {
    let src = loader.load(name)?;
    let tokens = tokenize(&src);
    Some(render_tokens(&tokens, loader, context, strip, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct InMemory(HashMap<String, String>);
    impl TemplateLoader for InMemory {
        fn load(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
    }

    #[test]
    fn substitutes_known_keys() {
        let templates = HashMap::from([
            ("hi.md".to_string(), "Hello, {{ name }}!\n".to_string()),
        ]);
        let loader = InMemory(templates);
        let ctx = HashMap::from([("name".to_string(), "world".to_string())]);
        assert_eq!(
            render_template(&loader, "hi.md", &ctx, true).unwrap(),
            "Hello, world!"
        );
    }

    #[test]
    fn include_support() {
        let templates = HashMap::from([
            ("main.md".to_string(), "Before{% include 'snippet.md' %}After".to_string()),
            ("snippet.md".to_string(), " [INCLUDED] ".to_string()),
        ]);
        let loader = InMemory(templates);
        let ctx = HashMap::new();
        assert_eq!(
            render_template(&loader, "main.md", &ctx, false).unwrap(),
            "Before [INCLUDED] After"
        );
    }

    #[test]
    fn if_block_basic() {
        let templates = HashMap::from([
            ("test.md".to_string(), "{% if show %}visible{% endif %}".to_string()),
        ]);
        let loader = InMemory(templates);

        let ctx = HashMap::from([("show".to_string(), "true".to_string())]);
        assert_eq!(
            render_template(&loader, "test.md", &ctx, false).unwrap(),
            "visible"
        );

        let ctx = HashMap::from([("show".to_string(), "false".to_string())]);
        assert_eq!(
            render_template(&loader, "test.md", &ctx, false).unwrap(),
            ""
        );
    }

    #[test]
    fn if_else_support() {
        let templates = HashMap::from([
            ("test.md".to_string(), "{% if mode %}A{% else %}B{% endif %}".to_string()),
        ]);
        let loader = InMemory(templates);

        let ctx = HashMap::from([("mode".to_string(), "x".to_string())]);
        assert_eq!(
            render_template(&loader, "test.md", &ctx, false).unwrap(),
            "A"
        );

        let ctx = HashMap::from([("mode".to_string(), "".to_string())]);
        assert_eq!(
            render_template(&loader, "test.md", &ctx, false).unwrap(),
            "B"
        );
    }

    #[test]
    fn if_elif_else_support() {
        let src = "{% if channel == 'telegram' %}TG{% elif channel == 'discord' %}DC{% else %}Other{% endif %}";

        let templates = HashMap::from([
            ("test.md".to_string(), src.to_string()),
        ]);
        let loader = InMemory(templates);

        let ctx = HashMap::from([("channel".to_string(), "telegram".to_string())]);
        let result = render_template(&loader, "test.md", &ctx, false).unwrap();
        assert_eq!(result, "TG", "expected TG but got '{}'", result);

        let ctx = HashMap::from([("channel".to_string(), "discord".to_string())]);
        let result = render_template(&loader, "test.md", &ctx, false).unwrap();
        assert_eq!(result, "DC", "expected DC but got '{}'", result);

        let ctx = HashMap::from([("channel".to_string(), "email".to_string())]);
        let result = render_template(&loader, "test.md", &ctx, false).unwrap();
        assert_eq!(result, "Other", "expected Other but got '{}'", result);
    }

    #[test]
    fn if_with_or_operator() {
        let templates = HashMap::from([
            ("test.md".to_string(), "{% if channel == 'telegram' or channel == 'qq' or channel == 'discord' %}chat{% endif %}".to_string()),
        ]);
        let loader = InMemory(templates);

        let ctx = HashMap::from([("channel".to_string(), "telegram".to_string())]);
        assert_eq!(
            render_template(&loader, "test.md", &ctx, false).unwrap(),
            "chat"
        );

        let ctx = HashMap::from([("channel".to_string(), "email".to_string())]);
        assert_eq!(
            render_template(&loader, "test.md", &ctx, false).unwrap(),
            ""
        );
    }

    #[test]
    fn raw_block() {
        let templates = HashMap::from([
            ("test.md".to_string(), "{% raw %}{{ not_replaced }}{% endraw %}".to_string()),
        ]);
        let loader = InMemory(templates);
        let ctx = HashMap::new();
        assert_eq!(
            render_template(&loader, "test.md", &ctx, false).unwrap(),
            "{{ not_replaced }}"
        );
    }

    #[test]
    fn nested_if_blocks() {
        let templates = HashMap::from([
            ("test.md".to_string(), "{% if a %}{% if b %}both{% else %}only a{% endif %}{% endif %}".to_string()),
        ]);
        let loader = InMemory(templates);

        let ctx = HashMap::from([
            ("a".to_string(), "1".to_string()),
            ("b".to_string(), "1".to_string()),
        ]);
        assert_eq!(
            render_template(&loader, "test.md", &ctx, false).unwrap(),
            "both"
        );

        let ctx = HashMap::from([
            ("a".to_string(), "1".to_string()),
            ("b".to_string(), "".to_string()),
        ]);
        assert_eq!(
            render_template(&loader, "test.md", &ctx, false).unwrap(),
            "only a"
        );
    }

    #[test]
    fn include_with_variables() {
        let templates = HashMap::from([
            ("main.md".to_string(), "Hello, {% include 'greeting.md' %}!".to_string()),
            ("greeting.md".to_string(), "{{ name }}".to_string()),
        ]);
        let loader = InMemory(templates);
        let ctx = HashMap::from([("name".to_string(), "World".to_string())]);
        assert_eq!(
            render_template(&loader, "main.md", &ctx, false).unwrap(),
            "Hello, World!"
        );
    }
}
