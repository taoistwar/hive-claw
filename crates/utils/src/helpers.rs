//! Generic helpers (port of `nanobot.utils.helpers`).
//!
//! Notable differences from the Python original:
//!
//! * `estimate_prompt_tokens` uses a char-based heuristic instead of
//!   `tiktoken` — we do not want to pull a Python-only tokenizer crate.
//!   Callers can plug in a real tokenizer via [`estimate_prompt_tokens_chain`]
//!   (see the `TokenCounter` trait).
//! * `sync_workspace_templates` is not ported here — templates should be
//!   bundled via the `skills` / dedicated templates crate.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use chrono::Local;
use chrono_tz::Tz;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Think-block stripping
// ---------------------------------------------------------------------------

static RE_THINK_BLOCK: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<think>.*?</think>").unwrap());
static RE_THINK_TRAIL: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)^\s*<think>.*$").unwrap());
static RE_THOUGHT_BLOCK: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?s)<thought>.*?</thought>").unwrap());
static RE_THOUGHT_TRAIL: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)^\s*<thought>.*$").unwrap());
static RE_THINK_MALFORMED: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<think(?:[^A-Za-z0-9_\-:>/]|$)").unwrap());
static RE_THOUGHT_MALFORMED: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<thought(?:[^A-Za-z0-9_\-:>/]|$)").unwrap());
static RE_THINK_CLOSE_START: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*</think>\s*").unwrap());
static RE_THINK_CLOSE_END: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s*</think>\s*$").unwrap());
static RE_THOUGHT_CLOSE_START: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*</thought>\s*").unwrap());
static RE_THOUGHT_CLOSE_END: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s*</thought>\s*$").unwrap());
static RE_CHANNEL_MARKER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*<\|?channel\|?>\s*").unwrap());
/// Partial control tags that may appear at the end of a stream chunk.
static RE_PARTIAL_CONTROL_TAG: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?m)(</?(?:t|th|thi|thin|think|tho|thou|thoug|though|thought)>?|<\|?(?:c|ch|cha|chan|chann|channe|channel)(?:\|?>?)?)\s*$"
    )
    .unwrap()
});

static RE_EXTRACT_THINK: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?s)<think>(.*?)</think>").unwrap());
static RE_EXTRACT_THOUGHT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?s)<thought>(.*?)</thought>").unwrap());

/// Remove thinking blocks, unclosed trailing tags, and tokenizer-level
/// template leaks.
pub fn strip_think(text: &str) -> String {
    // The regex `<think(?![A-Za-z0-9_\-:>/])` uses a negative look-ahead that
    // Rust's `regex` crate does not support. We emulate it: match
    // `<think` followed by either EOF or a char that is *not* in the allowed
    // tag-name set, and only remove the `<think` portion.
    let mut s: String = text.to_string();
    s = RE_THINK_BLOCK.replace_all(&s, "").into_owned();
    s = RE_THINK_TRAIL.replace_all(&s, "").into_owned();
    s = RE_THOUGHT_BLOCK.replace_all(&s, "").into_owned();
    s = RE_THOUGHT_TRAIL.replace_all(&s, "").into_owned();
    s = RE_THINK_MALFORMED
        .replace_all(&s, |caps: &regex::Captures| {
            // Preserve the trailing sentinel char when present (it was a
            // look-ahead in Python).
            let whole = &caps[0];
            whole.chars().skip("<think".len()).collect::<String>()
        })
        .into_owned();
    s = RE_THOUGHT_MALFORMED
        .replace_all(&s, |caps: &regex::Captures| {
            let whole = &caps[0];
            whole.chars().skip("<thought".len()).collect::<String>()
        })
        .into_owned();
    s = RE_THINK_CLOSE_START.replace_all(&s, "").into_owned();
    s = RE_THINK_CLOSE_END.replace_all(&s, "").into_owned();
    s = RE_THOUGHT_CLOSE_START.replace_all(&s, "").into_owned();
    s = RE_THOUGHT_CLOSE_END.replace_all(&s, "").into_owned();
    s = RE_CHANNEL_MARKER.replace_all(&s, "").into_owned();
    // Stream chunks may end in the middle of a control tag.
    // Strip trailing partial control tags like </thi, <channe|, etc.
    let partial_tag = RE_PARTIAL_CONTROL_TAG.replace_all(&s, "");
    // Strip edge-case `<|` prefix at the start of the string.
    let partial_tag = regex::Regex::new(r"^\s*<\|?$").unwrap().replace_all(&partial_tag, "");
    partial_tag.trim().to_string()
}

/// Extract thinking content from inline `<think>` / `<thought>` blocks.
///
/// Returns `(thinking_text, cleaned_text)`. Only closed blocks are
/// extracted; unclosed streaming prefixes are stripped from the cleaned
/// text but not surfaced — [`strip_think`] handles that case.
pub fn extract_think(text: &str) -> (Option<String>, String) {
    let mut parts: Vec<String> = Vec::new();
    for m in RE_EXTRACT_THINK.captures_iter(text) {
        if let Some(inner) = m.get(1) {
            let trimmed = inner.as_str().trim().to_string();
            if !trimmed.is_empty() {
                parts.push(trimmed);
            }
        }
    }
    for m in RE_EXTRACT_THOUGHT.captures_iter(text) {
        if let Some(inner) = m.get(1) {
            let trimmed = inner.as_str().trim().to_string();
            if !trimmed.is_empty() {
                parts.push(trimmed);
            }
        }
    }
    let thinking = if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    };
    (thinking, strip_think(text))
}

/// Stateful inline `<think>` extractor for streaming buffers.
///
/// Streaming providers expose only a single content delta channel. When a
/// model embeds reasoning in `<think>...</think>` blocks inside that
/// channel, callers need to surface the reasoning incrementally as it
/// arrives without re-emitting earlier text. This holds the "already
/// emitted" cursor so the runner and the loop hook share one shape.
pub struct IncrementalThinkExtractor {
    emitted: String,
}

impl IncrementalThinkExtractor {
    pub fn new() -> Self {
        Self {
            emitted: String::new(),
        }
    }

    pub fn reset(&mut self) {
        self.emitted.clear();
    }

    /// Emit any new thinking text found in `buf`.
    ///
    /// Returns `Some(new_text)` if anything was emitted this call, or `None`
    /// otherwise. The caller is responsible for forwarding the new text to
    /// whatever reasoning hook / callback mechanism they use.
    pub fn feed(&mut self, buf: &str) -> Option<String> {
        let (thinking, _) = extract_think(buf);
        let thinking = thinking?;
        if thinking == self.emitted {
            return None;
        }
        let new = if thinking.len() > self.emitted.len() {
            thinking[self.emitted.len()..].trim().to_string()
        } else {
            thinking.trim().to_string()
        };
        self.emitted = thinking;
        if new.is_empty() {
            None
        } else {
            Some(new)
        }
    }
}

/// Return `(reasoning_text, cleaned_content)` from one model response.
///
/// Single source of truth for "what reasoning did this response carry, and
/// what answer text remains after we peel it out". Fallback order:
///
/// 1. Dedicated `reasoning_content` (DeepSeek-R1, Kimi, MiMo, OpenAI
///    reasoning models, Bedrock).
/// 2. Anthropic `thinking_blocks`.
/// 3. Inline `<think>` / `<thought>` blocks in `content`.
///
/// Only one source contributes per response; lower-priority sources are
/// ignored if a higher-priority one is present, but inline `<think>`
/// tags are still stripped from `content` so they never leak into the
/// final answer.
pub fn extract_reasoning(
    reasoning_content: Option<&str>,
    thinking_blocks: Option<&[Value]>,
    content: Option<&str>,
) -> (Option<String>, Option<String>) {
    if let Some(rc) = reasoning_content {
        if !rc.is_empty() {
            let cleaned = content.map(|c| strip_think(c));
            return (Some(rc.to_string()), cleaned);
        }
    }
    if let Some(blocks) = thinking_blocks {
        let parts: Vec<String> = blocks
            .iter()
            .filter_map(|tb| {
                let obj = tb.as_object()?;
                if obj.get("type").and_then(Value::as_str) != Some("thinking") {
                    return None;
                }
                let thinking = obj.get("thinking").and_then(Value::as_str)?;
                if thinking.is_empty() {
                    None
                } else {
                    Some(thinking.to_string())
                }
            })
            .collect();
        let joined = if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n"))
        };
        let cleaned = content.map(|c| strip_think(c));
        return (joined, cleaned);
    }
    if let Some(c) = content {
        let (thinking, cleaned) = extract_think(c);
        return (thinking, if cleaned.is_empty() { None } else { Some(cleaned) });
    }
    (None, content.map(|s| s.to_string()))
}

// ---------------------------------------------------------------------------
// Image helpers
// ---------------------------------------------------------------------------

/// Detect image MIME type from magic bytes, ignoring file extension.
pub fn detect_image_mime(data: &[u8]) -> Option<&'static str> {
    if data.len() >= 8 && data[..8] == *b"\x89PNG\r\n\x1a\n" {
        return Some("image/png");
    }
    if data.len() >= 3 && data[..3] == *b"\xff\xd8\xff" {
        return Some("image/jpeg");
    }
    if data.len() >= 6 && (&data[..6] == b"GIF87a" || &data[..6] == b"GIF89a") {
        return Some("image/gif");
    }
    if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// Build native image blocks plus a short text label.
pub fn build_image_content_blocks(
    raw: &[u8],
    mime: &str,
    path: &str,
    label: &str,
) -> Vec<Value> {
    let b64 = B64.encode(raw);
    vec![
        serde_json::json!({
            "type": "image_url",
            "image_url": { "url": format!("data:{mime};base64,{b64}") },
            "_meta": { "path": path },
        }),
        serde_json::json!({ "type": "text", "text": label }),
    ]
}

// ---------------------------------------------------------------------------
// Path / filesystem helpers
// ---------------------------------------------------------------------------

/// Ensure a directory exists and return its path.
pub fn ensure_dir(path: &Path) -> std::io::Result<PathBuf> {
    fs::create_dir_all(path)?;
    Ok(path.to_path_buf())
}

static UNSAFE_CHARS: Lazy<Regex> = Lazy::new(|| Regex::new(r#"[<>:"/\\|?*]"#).unwrap());

/// Replace unsafe path characters with underscores.
pub fn safe_filename(name: &str) -> String {
    UNSAFE_CHARS.replace_all(name, "_").trim().to_string()
}

// ---------------------------------------------------------------------------
// Time helpers
// ---------------------------------------------------------------------------

/// Current ISO-8601 timestamp (local time, matching Python's
/// `datetime.now().isoformat()`).
pub fn timestamp() -> String {
    Local::now().to_rfc3339()
}

/// Return the current time string with timezone info.
pub fn current_time_str(timezone: Option<&str>) -> String {
    match timezone.and_then(|tz| tz.parse::<Tz>().ok()) {
        Some(tz) => {
            let now = chrono::Utc::now().with_timezone(&tz);
            format_time(now.format("%Y-%m-%d %H:%M (%A)").to_string(), &now.format("%z").to_string(), timezone.unwrap_or("UTC"))
        }
        None => {
            let now = Local::now();
            let tz_name = std::env::var("TZ")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "UTC".into());
            let date = now.format("%Y-%m-%d %H:%M (%A)").to_string();
            let offset = now.format("%z").to_string();
            format_time(date, &offset, &tz_name)
        }
    }
}

fn format_time(date: String, offset: &str, tz_name: &str) -> String {
    let offset_fmt = if offset.len() == 5 {
        format!("{}:{}", &offset[..3], &offset[3..])
    } else {
        offset.to_string()
    };
    format!("{date} ({tz_name}, UTC{offset_fmt})")
}

// ---------------------------------------------------------------------------
// Message shaping
// ---------------------------------------------------------------------------

const TOOL_RESULT_PREVIEW_CHARS: usize = 1200;
const TOOL_RESULTS_DIR: &str = ".nanobot/tool-results";
const TOOL_RESULT_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;
const TOOL_RESULT_MAX_BUCKETS: usize = 32;

/// Build an image placeholder string.
pub fn image_placeholder_text(path: Option<&str>) -> String {
    image_placeholder_text_with(path, "[image]")
}

pub fn image_placeholder_text_with(path: Option<&str>, empty: &str) -> String {
    match path {
        Some(p) if !p.is_empty() => format!("[image: {p}]"),
        _ => empty.to_string(),
    }
}

/// Truncate text with a stable suffix.
pub fn truncate_text(text: &str, max_chars: usize) -> String {
    if max_chars == 0 || text.chars().count() <= max_chars {
        return text.to_string();
    }
    let taken: String = text.chars().take(max_chars).collect();
    format!("{taken}\n... (truncated)")
}

/// Find the first index whose tool results have matching assistant calls.
pub fn find_legal_message_start(messages: &[Value]) -> usize {
    use std::collections::HashSet;
    let mut declared: HashSet<String> = HashSet::new();
    let mut start: usize = 0;
    for (i, msg) in messages.iter().enumerate() {
        let role = msg.get("role").and_then(Value::as_str);
        match role {
            Some("assistant") => {
                if let Some(tcs) = msg.get("tool_calls").and_then(Value::as_array) {
                    for tc in tcs {
                        if let Some(id) = tc.get("id").and_then(Value::as_str) {
                            declared.insert(id.to_string());
                        } else if let Some(id) = tc.get("id").and_then(Value::as_i64) {
                            declared.insert(id.to_string());
                        }
                    }
                }
            }
            Some("tool") => {
                let tid = msg
                    .get("tool_call_id")
                    .and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_i64().map(|n| n.to_string())))
                    .unwrap_or_default();
                if !tid.is_empty() && !declared.contains(&tid) {
                    start = i + 1;
                    declared.clear();
                    // Re-scan preceding messages within the new window.
                    for prev in &messages[start..=i] {
                        if prev.get("role").and_then(Value::as_str) == Some("assistant") {
                            if let Some(tcs) = prev.get("tool_calls").and_then(Value::as_array) {
                                for tc in tcs {
                                    if let Some(id) = tc.get("id").and_then(Value::as_str) {
                                        declared.insert(id.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    start
}

/// Concatenate a sequence of `{ "type": "text", "text": "..." }` blocks,
/// returning `None` if any block is the wrong shape.
pub fn stringify_text_blocks(content: &[Value]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for block in content {
        let obj = block.as_object()?;
        if obj.get("type").and_then(Value::as_str) != Some("text") {
            return None;
        }
        let text = obj.get("text")?.as_str()?;
        parts.push(text.to_string());
    }
    Some(parts.join("\n"))
}

fn render_tool_result_reference(
    filepath: &Path,
    original_size: usize,
    preview: &str,
    truncated_preview: bool,
) -> String {
    let mut result = format!(
        "[tool output persisted]\n\
         Full output saved to: {fp}\n\
         Original size: {sz} chars\n\
         Preview:\n{preview}",
        fp = filepath.display(),
        sz = original_size,
    );
    if truncated_preview {
        result.push_str("\n...\n(Read the saved file if you need the full output.)");
    }
    result
}

fn bucket_mtime(path: &Path) -> u64 {
    path.metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cleanup_tool_result_buckets(root: &Path, current_bucket: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let cutoff = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .saturating_sub(TOOL_RESULT_RETENTION_SECS);

    let mut siblings: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path == current_bucket || !path.is_dir() {
            continue;
        }
        if bucket_mtime(&path) < cutoff {
            let _ = fs::remove_dir_all(&path);
        } else {
            siblings.push(path);
        }
    }
    let keep = TOOL_RESULT_MAX_BUCKETS.saturating_sub(1);
    siblings.retain(|p| p.exists());
    if siblings.len() <= keep {
        return;
    }
    siblings.sort_by_key(|p| std::cmp::Reverse(bucket_mtime(p)));
    for p in siblings.iter().skip(keep) {
        let _ = fs::remove_dir_all(p);
    }
}

fn write_text_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    let tmp_name = format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("tmp"),
        uuid::Uuid::new_v4().simple(),
    );
    let tmp = parent.join(tmp_name);
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
    }
    let result = fs::rename(&tmp, path);
    if result.is_err() && tmp.exists() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Persist oversized tool output and replace it with a stable reference.
///
/// `content` is expected to be either a plain string or a list of
/// `{type: text, text: ...}` blocks; anything else is returned unchanged.
pub fn maybe_persist_tool_result(
    workspace: Option<&Path>,
    session_key: Option<&str>,
    tool_call_id: &str,
    content: Value,
    max_chars: usize,
) -> Value {
    let Some(workspace) = workspace else {
        return content;
    };
    if max_chars == 0 {
        return content;
    }

    let (text_payload, suffix) = match &content {
        Value::String(s) => (s.clone(), "txt"),
        Value::Array(arr) => match stringify_text_blocks(arr) {
            Some(s) => (s, "json"),
            None => return content,
        },
        _ => return content,
    };

    if text_payload.chars().count() <= max_chars {
        return content;
    }

    let root = match ensure_dir(&workspace.join(TOOL_RESULTS_DIR)) {
        Ok(p) => p,
        Err(_) => return content,
    };
    let bucket_name = safe_filename(session_key.unwrap_or("default"));
    let bucket = match ensure_dir(&root.join(&bucket_name)) {
        Ok(p) => p,
        Err(_) => return content,
    };
    cleanup_tool_result_buckets(&root, &bucket);

    let path = bucket.join(format!("{}.{suffix}", safe_filename(tool_call_id)));
    if !path.exists() {
        let text_out = if suffix == "json" {
            match &content {
                Value::Array(arr) => {
                    serde_json::to_string_pretty(&Value::Array(arr.clone()))
                        .unwrap_or_else(|_| text_payload.clone())
                }
                _ => text_payload.clone(),
            }
        } else {
            text_payload.clone()
        };
        let _ = write_text_atomic(&path, &text_out);
    }

    let preview: String = text_payload.chars().take(TOOL_RESULT_PREVIEW_CHARS).collect();
    let original_size = text_payload.chars().count();
    Value::String(render_tool_result_reference(
        &path,
        original_size,
        &preview,
        original_size > TOOL_RESULT_PREVIEW_CHARS,
    ))
}

/// Split content into chunks within `max_len`, preferring line breaks /
/// spaces (used mainly for Discord-size compatibility).
pub fn split_message(content: &str, max_len: usize) -> Vec<String> {
    if content.is_empty() {
        return Vec::new();
    }
    if content.chars().count() <= max_len {
        return vec![content.to_string()];
    }
    let mut chunks: Vec<String> = Vec::new();
    let mut remaining = content.to_string();
    while !remaining.is_empty() {
        if remaining.chars().count() <= max_len {
            chunks.push(remaining);
            break;
        }
        // `rfind` works on bytes; we keep splits at char boundaries by using
        // `char_indices` to locate a byte-safe cut position.
        let byte_cut = nth_char_boundary(&remaining, max_len);
        let head = &remaining[..byte_cut];
        // Prefer the last newline, then the last space.
        let pos_byte = head
            .rfind('\n')
            .or_else(|| head.rfind(' '))
            .filter(|&p| p > 0)
            .unwrap_or(byte_cut);
        let (chunk, rest) = remaining.split_at(pos_byte);
        chunks.push(chunk.to_string());
        remaining = rest.trim_start().to_string();
    }
    chunks
}

fn nth_char_boundary(s: &str, char_count: usize) -> usize {
    s.char_indices()
        .nth(char_count)
        .map(|(i, _)| i)
        .unwrap_or_else(|| s.len())
}

/// Build a provider-safe assistant message with optional reasoning fields.
pub fn build_assistant_message(
    content: Option<&str>,
    tool_calls: Option<&[Value]>,
    reasoning_content: Option<&str>,
    thinking_blocks: Option<&[Value]>,
) -> Value {
    let mut msg = serde_json::Map::new();
    msg.insert("role".into(), Value::String("assistant".into()));
    msg.insert(
        "content".into(),
        Value::String(content.unwrap_or("").to_string()),
    );
    if let Some(tcs) = tool_calls {
        if !tcs.is_empty() {
            msg.insert("tool_calls".into(), Value::Array(tcs.to_vec()));
        }
    }
    if reasoning_content.is_some() || thinking_blocks.is_some() {
        msg.insert(
            "reasoning_content".into(),
            Value::String(reasoning_content.unwrap_or("").to_string()),
        );
    }
    if let Some(tb) = thinking_blocks {
        if !tb.is_empty() {
            msg.insert("thinking_blocks".into(), Value::Array(tb.to_vec()));
        }
    }
    Value::Object(msg)
}

// ---------------------------------------------------------------------------
// Token estimation (heuristic)
// ---------------------------------------------------------------------------

/// Rough per-char token estimator (tiktoken-free). Roughly matches GPT's
/// "one token ≈ 4 chars" rule of thumb.
fn char_based_token_estimate(text: &str) -> usize {
    (text.chars().count() + 3) / 4
}

/// Estimate prompt tokens. No `tiktoken` binding in Rust — we emit a
/// conservative char-based estimate. Callers wanting precision should use
/// [`estimate_prompt_tokens_chain`] with a real [`TokenCounter`].
pub fn estimate_prompt_tokens(messages: &[Value], tools: Option<&[Value]>) -> usize {
    let mut parts: Vec<String> = Vec::new();
    for msg in messages {
        if let Some(content) = msg.get("content") {
            match content {
                Value::String(s) => parts.push(s.clone()),
                Value::Array(arr) => {
                    for part in arr {
                        if part.get("type").and_then(Value::as_str) == Some("text") {
                            if let Some(t) = part.get("text").and_then(Value::as_str) {
                                if !t.is_empty() {
                                    parts.push(t.to_string());
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(tc) = msg.get("tool_calls") {
            if !tc.is_null() {
                parts.push(serde_json::to_string(tc).unwrap_or_default());
            }
        }
        if let Some(rc) = msg.get("reasoning_content").and_then(Value::as_str) {
            if !rc.is_empty() {
                parts.push(rc.to_string());
            }
        }
        for key in ["name", "tool_call_id"] {
            if let Some(v) = msg.get(key).and_then(Value::as_str) {
                if !v.is_empty() {
                    parts.push(v.to_string());
                }
            }
        }
    }
    if let Some(tools) = tools {
        parts.push(serde_json::to_string(tools).unwrap_or_default());
    }
    let per_message_overhead = messages.len() * 4;
    char_based_token_estimate(&parts.join("\n")) + per_message_overhead
}

/// Estimate prompt tokens contributed by a single persisted message.
pub fn estimate_message_tokens(message: &Value) -> usize {
    let mut parts: Vec<String> = Vec::new();
    if let Some(content) = message.get("content") {
        match content {
            Value::String(s) => parts.push(s.clone()),
            Value::Array(arr) => {
                for part in arr {
                    if part.get("type").and_then(Value::as_str) == Some("text") {
                        if let Some(t) = part.get("text").and_then(Value::as_str) {
                            if !t.is_empty() {
                                parts.push(t.to_string());
                            }
                        }
                    } else {
                        parts.push(serde_json::to_string(part).unwrap_or_default());
                    }
                }
            }
            Value::Null => {}
            other => parts.push(serde_json::to_string(other).unwrap_or_default()),
        }
    }
    for key in ["name", "tool_call_id"] {
        if let Some(v) = message.get(key).and_then(Value::as_str) {
            if !v.is_empty() {
                parts.push(v.to_string());
            }
        }
    }
    if let Some(tc) = message.get("tool_calls") {
        if !tc.is_null() {
            parts.push(serde_json::to_string(tc).unwrap_or_default());
        }
    }
    if let Some(rc) = message.get("reasoning_content").and_then(Value::as_str) {
        if !rc.is_empty() {
            parts.push(rc.to_string());
        }
    }
    let payload = parts.join("\n");
    if payload.is_empty() {
        return 4;
    }
    std::cmp::max(4, char_based_token_estimate(&payload) + 4)
}

/// Provider-supplied token counter. A concrete `LLMProvider` implementation
/// may offer a more accurate estimate than the char-based fallback above.
pub trait TokenCounter {
    fn estimate_prompt_tokens(
        &self,
        messages: &[Value],
        tools: Option<&[Value]>,
        model: Option<&str>,
    ) -> Option<(usize, String)>;
}

/// Estimate prompt tokens via provider counter first, falling back to the
/// built-in char-based heuristic.
pub fn estimate_prompt_tokens_chain<C: TokenCounter + ?Sized>(
    provider: Option<&C>,
    model: Option<&str>,
    messages: &[Value],
    tools: Option<&[Value]>,
) -> (usize, String) {
    if let Some(p) = provider {
        if let Some((tokens, source)) = p.estimate_prompt_tokens(messages, tools, model) {
            if tokens > 0 {
                let src = if source.is_empty() {
                    "provider_counter".into()
                } else {
                    source
                };
                return (tokens, src);
            }
        }
    }
    let estimated = estimate_prompt_tokens(messages, tools);
    if estimated > 0 {
        (estimated, "char_heuristic".into())
    } else {
        (0, "none".into())
    }
}

// ---------------------------------------------------------------------------
// Runtime status snapshot
// ---------------------------------------------------------------------------

/// Parameters for [`build_status_content`].
#[derive(Debug, Clone)]
pub struct StatusContent<'a> {
    pub version: &'a str,
    pub model: &'a str,
    /// Epoch seconds (as returned by `SystemTime::now().duration_since(UNIX_EPOCH)`).
    pub start_time: f64,
    pub last_usage: &'a std::collections::HashMap<String, u64>,
    pub context_window_tokens: u64,
    pub session_msg_count: usize,
    pub context_tokens_estimate: u64,
    pub search_usage_text: Option<&'a str>,
    pub active_task_count: usize,
    pub max_completion_tokens: u64,
}

/// Build a human-readable runtime status snapshot.
pub fn build_status_content(p: StatusContent<'_>) -> String {
    let now_s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let uptime_s = (now_s - p.start_time).max(0.0) as u64;
    let uptime = if uptime_s >= 3600 {
        format!("{}h {}m", uptime_s / 3600, (uptime_s % 3600) / 60)
    } else {
        format!("{}m {}s", uptime_s / 60, uptime_s % 60)
    };
    let last_in = *p.last_usage.get("prompt_tokens").unwrap_or(&0);
    let last_out = *p.last_usage.get("completion_tokens").unwrap_or(&0);
    let cached = *p.last_usage.get("cached_tokens").unwrap_or(&0);

    let ctx_total = p.context_window_tokens;
    // Mirror Consolidator formula: ctx - max_completion - safety_buffer.
    let ctx_budget = ctx_total.saturating_sub(p.max_completion_tokens).saturating_sub(1024).max(1);
    let ctx_pct = std::cmp::min(
        (p.context_tokens_estimate * 100 / ctx_budget) as u64,
        999,
    );

    let ctx_used_str = if p.context_tokens_estimate >= 1000 {
        format!("{}k", p.context_tokens_estimate / 1000)
    } else {
        p.context_tokens_estimate.to_string()
    };
    let ctx_total_str = if ctx_total > 0 {
        format!("{}k", ctx_total / 1000)
    } else {
        "n/a".to_string()
    };

    let mut token_line = format!("\u{1f4ca} Tokens: {last_in} in / {last_out} out");
    if cached > 0 && last_in > 0 {
        token_line.push_str(&format!(" ({}% cached)", cached * 100 / last_in));
    }
    let mut lines = vec![
        format!("\u{1f408} nanobot v{}", p.version),
        format!("\u{1f9e0} Model: {}", p.model),
        token_line,
        format!(
            "\u{1f4da} Context: {ctx_used_str}/{ctx_total_str} ({ctx_pct}% of input budget)"
        ),
        format!("\u{1f4ac} Session: {} messages", p.session_msg_count),
        format!("\u{23f1} Uptime: {uptime}"),
        format!("\u{26a1} Tasks: {} active", p.active_task_count),
    ];
    if let Some(text) = p.search_usage_text {
        lines.push(text.to_string());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_filename_replaces() {
        assert_eq!(safe_filename("a/b:c*?.txt"), "a_b_c__.txt");
    }

    #[test]
    fn strip_think_removes_block() {
        assert_eq!(strip_think("hi <think>x</think> you"), "hi  you");
    }

    #[test]
    fn detect_image_mime_png() {
        let bytes = b"\x89PNG\r\n\x1a\nrest";
        assert_eq!(detect_image_mime(bytes), Some("image/png"));
    }

    #[test]
    fn find_legal_message_start_skips_orphan() {
        let msgs = vec![
            serde_json::json!({ "role": "tool", "tool_call_id": "orphan" }),
            serde_json::json!({ "role": "assistant", "tool_calls": [{"id": "x"}] }),
            serde_json::json!({ "role": "tool", "tool_call_id": "x" }),
        ];
        assert_eq!(find_legal_message_start(&msgs), 1);
    }

    #[test]
    fn split_message_respects_max_len() {
        let s = "a".repeat(100);
        let chunks = split_message(&s, 40);
        assert!(chunks.len() >= 3);
        for c in &chunks {
            assert!(c.chars().count() <= 40);
        }
    }
}
