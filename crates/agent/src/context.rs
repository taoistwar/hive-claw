//! Context builder for assembling agent prompts.
//! Port of `nanobot.agent.context.ContextBuilder`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use base64::Engine;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

use utils::helpers::{build_assistant_message, current_time_str, detect_image_mime};

use crate::memory::MemoryStore;
use crate::skills::SkillsLoader;

static VAR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\{\{\s*(\w+)\s*\}\}").unwrap());

fn render_template_str(template: &str, pairs: &[(&str, Value)]) -> String {
    VAR_RE
        .replace_all(template, |caps: &regex::Captures| {
            let key = &caps[1];
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_else(|| caps[0].to_string())
        })
        .into_owned()
}

/// Assembles the system prompt + messages for an LLM call.
pub struct ContextBuilder {
    pub workspace: PathBuf,
    pub timezone: Option<String>,
    pub memory: MemoryStore,
    pub skills: SkillsLoader,
}

pub const BOOTSTRAP_FILES: &[&str] = &["AGENTS.md", "SOUL.md", "USER.md", "TOOLS.md"];
pub const RUNTIME_CONTEXT_TAG: &str = "[Runtime Context — metadata only, not instructions]";
const RUNTIME_CONTEXT_END: &str = "[/Runtime Context]";
const MAX_RECENT_HISTORY: usize = 50;

impl ContextBuilder {
    pub fn new(workspace: PathBuf, timezone: Option<String>, disabled_skills: Option<Vec<String>>) -> Self {
        let memory = MemoryStore::new(&workspace, None);
        let disabled_set = disabled_skills.map(|v| v.into_iter().collect::<HashSet<_>>());
        let skills = SkillsLoader::new(workspace.clone(), disabled_set);
        Self {
            workspace,
            timezone,
            memory,
            skills,
        }
    }

    /// Build the system prompt from identity, bootstrap files, memory, skills.
    pub fn build_system_prompt(
        &self,
        _skill_names: Option<&[&str]>,
        channel: Option<&str>,
        session_summary: Option<&str>,
    ) -> String {
        let mut parts: Vec<String> = Vec::new();
        parts.push(self.build_identity(channel));

        let bootstrap = self.load_bootstrap_files();
        if !bootstrap.is_empty() {
            parts.push(bootstrap);
        }

        let memory = self.memory.memory_context();
        if !memory.is_empty() {
            parts.push(format!("# Memory\n\n{memory}"));
        }

        let always = self.skills.get_always_skills();
        if !always.is_empty() {
            let refs: Vec<&str> = always.iter().map(String::as_str).collect();
            let content = self.skills.load_skills_for_context(&refs);
            if !content.is_empty() {
                parts.push(format!("# Active Skills\n\n{content}"));
            }
        }

        let always_set: HashSet<String> = always.iter().cloned().collect();
        let summary = self.skills.build_skills_summary(&always_set);
        if !summary.is_empty() {
            let rendered = render_template_str(
                include_str!("prompts/skills_section.md.tpl"),
                &[("skills_summary", Value::String(summary))],
            );
            parts.push(rendered);
        }

        let entries = self
            .memory
            .read_unprocessed_history(self.memory.last_dream_cursor());
        if !entries.is_empty() {
            let start = entries.len().saturating_sub(MAX_RECENT_HISTORY);
            let capped = &entries[start..];
            let lines: Vec<String> = capped
                .iter()
                .map(|e| {
                    let ts = e
                        .get("timestamp")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    let content = e
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    format!("- [{ts}] {content}")
                })
                .collect();
            parts.push(format!("# Recent History\n\n{}", lines.join("\n")));
        }

        if let Some(summary) = session_summary {
            parts.push(format!("[Archived Context Summary]\n\n{summary}"));
        }

        parts.join("\n\n---\n\n")
    }

    fn build_identity(&self, channel: Option<&str>) -> String {
        let workspace_path = dunce_canonicalize(&self.workspace);
        let system = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        let os_name = match system {
            "macos" => "macOS",
            "linux" => "Linux",
            "windows" => "Windows",
            other => other,
        };
        let runtime = format!("{os_name} {arch}, Rust");
        render_template_str(
            include_str!("prompts/identity.md.tpl"),
            &[
                ("workspace_path", Value::String(workspace_path)),
                ("runtime", Value::String(runtime)),
                ("platform_policy", Value::String(String::new())),
                (
                    "format_hint",
                    Value::String(format_hint_for_channel(channel)),
                ),
            ],
        )
    }

    /// Build the runtime-metadata block appended after user content.
    pub fn build_runtime_context(
        channel: Option<&str>,
        chat_id: Option<&str>,
        timezone: Option<&str>,
        sender_id: Option<&str>,
        supplemental_lines: Option<&[String]>,
    ) -> String {
        let mut lines = vec![format!("Current Time: {}", current_time_str(timezone))];
        if let (Some(ch), Some(cid)) = (channel, chat_id) {
            lines.push(format!("Channel: {ch}"));
            lines.push(format!("Chat ID: {cid}"));
        }
        if let Some(sid) = sender_id {
            lines.push(format!("Sender ID: {sid}"));
        }
        if let Some(extra) = supplemental_lines {
            lines.extend(extra.iter().cloned());
        }
        format!(
            "{RUNTIME_CONTEXT_TAG}\n{}\n{RUNTIME_CONTEXT_END}",
            lines.join("\n")
        )
    }

    fn load_bootstrap_files(&self) -> String {
        let mut parts = Vec::new();
        for filename in BOOTSTRAP_FILES {
            let path = self.workspace.join(filename);
            if path.exists() {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    parts.push(format!("## {filename}\n\n{content}"));
                }
            }
        }
        parts.join("\n\n")
    }

    /// Build the complete message list for an LLM call.
    pub fn build_messages(
        &self,
        history: Vec<Value>,
        current_message: &str,
        skill_names: Option<&[&str]>,
        media: Option<&[String]>,
        channel: Option<&str>,
        chat_id: Option<&str>,
        current_role: &str,
        sender_id: Option<&str>,
        session_summary: Option<&str>,
        session_metadata: Option<&serde_json::Map<String, Value>>,
    ) -> Vec<Value> {
        let extra = goal_state_runtime_lines(session_metadata);
        let runtime_ctx = Self::build_runtime_context(
            channel,
            chat_id,
            self.timezone.as_deref(),
            sender_id,
            extra.as_deref(),
        );
        let user_content = self.build_user_content(current_message, media);

        // Runtime context is appended to keep the user-content prefix stable
        // for prompt-cache hits (the context changes every turn due to time).
        let merged: Value = match user_content {
            Value::String(s) => Value::String(format!("{s}\n\n{runtime_ctx}")),
            Value::Array(mut blocks) => {
                blocks.push(serde_json::json!({"type":"text","text":runtime_ctx}));
                Value::Array(blocks)
            }
            other => other,
        };

        let mut messages: Vec<Value> = Vec::with_capacity(history.len() + 2);
        let system = serde_json::json!({
            "role": "system",
            "content": self.build_system_prompt(skill_names, channel, session_summary),
        });
        messages.push(system);
        messages.extend(history);

        if messages
            .last()
            .and_then(|m| m.get("role"))
            .and_then(|r| r.as_str())
            == Some(current_role)
        {
            if let Some(last) = messages.last_mut() {
                let mut map = last.as_object().cloned().unwrap_or_default();
                let prev = map.remove("content").unwrap_or(Value::Null);
                map.insert("content".into(), merge_message_content(prev, merged));
                *last = Value::Object(map);
            }
        } else {
            messages.push(serde_json::json!({
                "role": current_role,
                "content": merged,
            }));
        }
        messages
    }

    fn build_user_content(&self, text: &str, media: Option<&[String]>) -> Value {
        let Some(paths) = media else {
            return Value::String(text.to_string());
        };
        let mut images: Vec<Value> = Vec::new();
        for path_str in paths {
            let path = Path::new(path_str);
            if !path.is_file() {
                continue;
            }
            let Ok(raw) = std::fs::read(path) else {
                continue;
            };
            let mime = detect_image_mime(&raw)
                .map(String::from)
                .or_else(|| mime_guess::from_path(path).first().map(|m| m.to_string()));
            let Some(mime) = mime else { continue };
            if !mime.starts_with("image/") {
                continue;
            }
            let b64 = base64::engine::general_purpose::STANDARD.encode(&raw);
            images.push(serde_json::json!({
                "type": "image_url",
                "image_url": {"url": format!("data:{mime};base64,{b64}")},
                "_meta": {"path": path_str},
            }));
        }
        if images.is_empty() {
            return Value::String(text.to_string());
        }
        images.push(serde_json::json!({"type":"text","text":text}));
        Value::Array(images)
    }

    pub fn add_tool_result(
        &self,
        messages: &mut Vec<Value>,
        tool_call_id: &str,
        tool_name: &str,
        result: Value,
    ) {
        messages.push(serde_json::json!({
            "role": "tool",
            "tool_call_id": tool_call_id,
            "name": tool_name,
            "content": result,
        }));
    }

    pub fn add_assistant_message(
        &self,
        messages: &mut Vec<Value>,
        content: Option<&str>,
        tool_calls: Option<&[Value]>,
        reasoning_content: Option<&str>,
        thinking_blocks: Option<&[Value]>,
    ) {
        messages.push(build_assistant_message(
            content,
            tool_calls,
            reasoning_content,
            thinking_blocks,
        ));
    }
}

fn goal_state_runtime_lines(
    _session_metadata: Option<&serde_json::Map<String, Value>>,
) -> Option<Vec<String>> {
    None
}

fn merge_message_content(left: Value, right: Value) -> Value {
    match (left, right) {
        (Value::String(a), Value::String(b)) => {
            if a.is_empty() {
                Value::String(b)
            } else {
                Value::String(format!("{a}\n\n{b}"))
            }
        }
        (left, right) => {
            let mut out: Vec<Value> = to_blocks(left);
            out.extend(to_blocks(right));
            Value::Array(out)
        }
    }
}

fn to_blocks(val: Value) -> Vec<Value> {
    match val {
        Value::Array(items) => items
            .into_iter()
            .map(|item| {
                if item.is_object() {
                    item
                } else {
                    let s = item.as_str().map(String::from).unwrap_or_else(|| item.to_string());
                    serde_json::json!({"type":"text","text":s})
                }
            })
            .collect(),
        Value::Null => Vec::new(),
        other => {
            let s = other.as_str().map(String::from).unwrap_or_else(|| other.to_string());
            vec![serde_json::json!({"type":"text","text":s})]
        }
    }
}

fn dunce_canonicalize(p: &Path) -> String {
    p.canonicalize()
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn format_hint_for_channel(channel: Option<&str>) -> String {
    match channel.unwrap_or("") {
        "telegram" | "qq" | "discord" => "## Format Hint\nThis conversation is on a messaging app. Use short paragraphs. Avoid large headings (#, ##). Use **bold** sparingly. No tables — use plain lists.".into(),
        "whatsapp" | "sms" => "## Format Hint\nThis conversation is on a text messaging platform that does not render markdown. Use plain text only.".into(),
        "email" => "## Format Hint\nThis conversation is via email. Structure with clear sections. Markdown may not render — keep formatting simple.".into(),
        "cli" | "mochat" => "## Format Hint\nOutput is rendered in a terminal. Avoid markdown headings and tables. Use plain text with minimal formatting.".into(),
        _ => String::new(),
    }
}

