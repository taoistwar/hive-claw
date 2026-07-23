//! Feishu/Lark channel implementation using lark-oapi SDK with WebSocket long connection.
//!
//! Rust port of `nanobot.channels.feishu`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use log::{debug, error, info, warn};
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::base::{Channel, ChannelError, ChannelResult, TranscriptionSettings, handle_inbound};
use bus::OutboundMessage;
use config::paths::get_media_dir;
use utils::helpers::safe_filename;

// ---------------------------------------------------------------------------
// Module-level constants (mirrors Python `FEISHU_AVAILABLE`, `MSG_TYPE_MAP`)
// ---------------------------------------------------------------------------

/// Placeholder for SDK availability check.
/// TODO: Set to true when lark-oapi Rust SDK is integrated.
const FEISHU_AVAILABLE: bool = false;

/// Message type display mapping.
#[expect(
    dead_code,
    reason = "reserved for inbound events once the Lark SDK placeholder is connected"
)]
const MSG_TYPE_MAP: &[(&str, &str)] = &[
    ("image", "[image]"),
    ("audio", "[audio]"),
    ("file", "[file]"),
    ("sticker", "[sticker]"),
];

#[expect(
    dead_code,
    reason = "reserved for inbound events once the Lark SDK placeholder is connected"
)]
fn msg_type_display(msg_type: &str) -> &str {
    MSG_TYPE_MAP
        .iter()
        .find(|(k, _)| *k == msg_type)
        .map(|(_, v)| *v)
        .unwrap_or(msg_type)
}

// ---------------------------------------------------------------------------
// Content extraction functions (mirrors Python `_extract_*`)
// ---------------------------------------------------------------------------

/// Extract text representation from share cards and interactive messages.
fn _extract_share_card_content(content_json: &serde_json::Value, msg_type: &str) -> String {
    let mut parts = Vec::new();

    match msg_type {
        "share_chat" => {
            let chat_id = content_json
                .get("chat_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            parts.push(format!("[shared chat: {}]", chat_id));
        }
        "share_user" => {
            let user_id = content_json
                .get("user_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            parts.push(format!("[shared user: {}]", user_id));
        }
        "interactive" => {
            parts.extend(_extract_interactive_content(content_json));
        }
        "share_calendar_event" => {
            let event_key = content_json
                .get("event_key")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            parts.push(format!("[shared calendar event: {}]", event_key));
        }
        "system" => {
            parts.push("[system message]".to_string());
        }
        "merge_forward" => {
            parts.push("[merged forward messages]".to_string());
        }
        _ => {}
    }

    if parts.is_empty() {
        format!("[{}]", msg_type)
    } else {
        parts.join("\n")
    }
}

/// Recursively extract text and links from interactive card content.
fn _extract_interactive_content(content: &serde_json::Value) -> Vec<String> {
    let mut parts = Vec::new();

    // If it's a string, try to parse as JSON
    if let Some(s) = content.as_str() {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
            return _extract_interactive_content(&parsed);
        }
        if !s.trim().is_empty() {
            return vec![s.to_string()];
        }
        return parts;
    }

    let obj = match content.as_object() {
        Some(o) => o,
        None => return parts,
    };

    // Title
    if let Some(title) = obj.get("title") {
        if let Some(title_obj) = title.as_object() {
            let title_content = title_obj
                .get("content")
                .or_else(|| title_obj.get("text"))
                .and_then(|v| v.as_str());
            if let Some(tc) = title_content.filter(|text| !text.is_empty()) {
                parts.push(format!("title: {}", tc));
            }
        } else if let Some(s) = title.as_str() {
            parts.push(format!("title: {}", s));
        }
    }

    // Elements
    if let Some(elements_arr) = obj.get("elements").and_then(|v| v.as_array()) {
        for element in elements_arr {
            if let Some(elem_obj) = element.as_object() {
                parts.extend(_extract_element_content(elem_obj));
            }
        }
    }

    // Nested card
    if let Some(card) = obj.get("card") {
        parts.extend(_extract_interactive_content(card));
    }

    // Header title
    if let Some(header_title) = obj
        .get("header")
        .and_then(|v| v.as_object())
        .and_then(|header| header.get("title"))
        .and_then(|title| title.as_object())
    {
        let header_text = header_title
            .get("content")
            .or_else(|| header_title.get("text"))
            .and_then(|v| v.as_str())
            .filter(|text| !text.is_empty());
        if let Some(ht) = header_text {
            parts.push(format!("title: {}", ht));
        }
    }

    parts
}

/// Extract content from a single card element.
fn _extract_element_content(element: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
    let mut parts = Vec::new();

    let tag = element.get("tag").and_then(|v| v.as_str()).unwrap_or("");

    match tag {
        "markdown" | "lark_md" => {
            if let Some(content) = element
                .get("content")
                .and_then(|v| v.as_str())
                .filter(|content| !content.is_empty())
            {
                parts.push(content.to_string());
            }
        }
        "div" => {
            if let Some(text) = element.get("text") {
                if let Some(text_obj) = text.as_object() {
                    let text_content = text_obj
                        .get("content")
                        .or_else(|| text_obj.get("text"))
                        .and_then(|v| v.as_str());
                    if let Some(tc) = text_content.filter(|text| !text.is_empty()) {
                        parts.push(tc.to_string());
                    }
                } else if let Some(s) = text.as_str() {
                    parts.push(s.to_string());
                }
            }
            if let Some(fields) = element.get("fields").and_then(|v| v.as_array()) {
                for field in fields {
                    let content = field
                        .as_object()
                        .and_then(|field| field.get("text"))
                        .and_then(|text| text.as_object())
                        .and_then(|text| text.get("content"))
                        .and_then(|content| content.as_str())
                        .filter(|content| !content.is_empty());
                    if let Some(content) = content {
                        parts.push(content.to_string());
                    }
                }
            }
        }
        "a" => {
            if let Some(href) = element
                .get("href")
                .and_then(|v| v.as_str())
                .filter(|href| !href.is_empty())
            {
                parts.push(format!("link: {}", href));
            }
            if let Some(text) = element
                .get("text")
                .and_then(|v| v.as_str())
                .filter(|text| !text.is_empty())
            {
                parts.push(text.to_string());
            }
        }
        "button" => {
            let content = element
                .get("text")
                .and_then(|text| text.as_object())
                .and_then(|text| text.get("content"))
                .and_then(|content| content.as_str())
                .filter(|content| !content.is_empty());
            if let Some(content) = content {
                parts.push(content.to_string());
            }
            let url = element.get("url").and_then(|v| v.as_str()).or_else(|| {
                element
                    .get("multi_url")
                    .and_then(|v| v.as_object())
                    .and_then(|o| o.get("url"))
                    .and_then(|v| v.as_str())
            });
            if let Some(url) = url.filter(|url| !url.is_empty()) {
                parts.push(format!("link: {}", url));
            }
        }
        "img" => {
            let alt = element
                .get("alt")
                .and_then(|v| v.as_object())
                .and_then(|o| o.get("content"))
                .and_then(|v| v.as_str())
                .unwrap_or("[image]");
            parts.push(alt.to_string());
        }
        "note" => {
            if let Some(elements) = element.get("elements").and_then(|v| v.as_array()) {
                for ne in elements {
                    if let Some(ne_obj) = ne.as_object() {
                        parts.extend(_extract_element_content(ne_obj));
                    }
                }
            }
        }
        "column_set" => {
            if let Some(columns) = element.get("columns").and_then(|v| v.as_array()) {
                for col in columns {
                    if let Some(column_elements) = col
                        .as_object()
                        .and_then(|column| column.get("elements"))
                        .and_then(|elements| elements.as_array())
                    {
                        for element in column_elements {
                            if let Some(element) = element.as_object() {
                                parts.extend(_extract_element_content(element));
                            }
                        }
                    }
                }
            }
        }
        "plain_text" => {
            if let Some(content) = element
                .get("content")
                .and_then(|v| v.as_str())
                .filter(|content| !content.is_empty())
            {
                parts.push(content.to_string());
            }
        }
        _ => {
            // Fallback: recursively extract nested elements
            if let Some(nested) = element.get("elements").and_then(|v| v.as_array()) {
                for ne in nested {
                    if let Some(ne_obj) = ne.as_object() {
                        parts.extend(_extract_element_content(ne_obj));
                    }
                }
            }
        }
    }

    parts
}

/// Extract text and image keys from Feishu post (rich text) message.
fn _extract_post_content(content_json: &serde_json::Value) -> (String, Vec<String>) {
    fn parse_block(
        block: &serde_json::Map<String, serde_json::Value>,
    ) -> (Option<String>, Vec<String>) {
        let content_arr = match block.get("content").and_then(|v| v.as_array()) {
            Some(c) => c,
            None => {
                return (
                    block
                        .get("title")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    Vec::new(),
                );
            }
        };

        let mut texts = Vec::new();
        let mut images = Vec::new();

        if let Some(title) = block.get("title").and_then(|v| v.as_str()) {
            texts.push(title.to_string());
        }

        for row in content_arr {
            let row_arr = match row.as_array() {
                Some(r) => r,
                None => continue,
            };
            for el in row_arr {
                let el_obj = match el.as_object() {
                    Some(e) => e,
                    None => continue,
                };
                let tag = el_obj.get("tag").and_then(|v| v.as_str()).unwrap_or("");
                match tag {
                    "text" | "a" => {
                        if let Some(t) = el_obj.get("text").and_then(|v| v.as_str()) {
                            texts.push(t.to_string());
                        }
                    }
                    "at" => {
                        let user_name = el_obj
                            .get("user_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("user");
                        texts.push(format!("@{}", user_name));
                    }
                    "code_block" => {
                        let lang = el_obj
                            .get("language")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let code_text = el_obj.get("text").and_then(|v| v.as_str()).unwrap_or("");
                        texts.push(format!("\n```\n{}\n{}\n```\n", lang, code_text));
                    }
                    "img" => {
                        if let Some(key) = el_obj.get("image_key").and_then(|v| v.as_str()) {
                            images.push(key.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }

        let text = texts.join(" ");
        let text = text.trim().to_string();
        if text.is_empty() && images.is_empty() {
            (None, images)
        } else {
            (Some(text), images)
        }
    }

    // Unwrap optional {"post": ...} envelope
    let root = if let Some(post_obj) = content_json.get("post").and_then(|v| v.as_object()) {
        post_obj
    } else {
        match content_json.as_object() {
            Some(o) => o,
            None => return (String::new(), Vec::new()),
        }
    };

    // Direct format
    if root.contains_key("content") {
        let (text, imgs) = parse_block(root);
        if text.is_some() || !imgs.is_empty() {
            return (text.unwrap_or_default(), imgs);
        }
    }

    // Localized: prefer known locales
    for key in &["zh_cn", "en_us", "ja_jp"] {
        if let Some(locale_obj) = root.get(*key).and_then(|v| v.as_object()) {
            let (text, imgs) = parse_block(locale_obj);
            if text.is_some() || !imgs.is_empty() {
                return (text.unwrap_or_default(), imgs);
            }
        }
    }

    // Fall back to any dict child
    for val in root.values() {
        if let Some(val_obj) = val.as_object() {
            let (text, imgs) = parse_block(val_obj);
            if text.is_some() || !imgs.is_empty() {
                return (text.unwrap_or_default(), imgs);
            }
        }
    }

    (String::new(), Vec::new())
}

/// Extract plain text from Feishu post (rich text) message content.
fn _extract_post_text(content_json: &serde_json::Value) -> String {
    let (text, _) = _extract_post_content(content_json);
    text
}

// ---------------------------------------------------------------------------
// Configuration struct (mirrors Python `FeishuConfig`)
// ---------------------------------------------------------------------------

/// Feishu/Lark channel configuration using WebSocket long connection.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct FeishuConfig {
    pub enabled: bool,
    pub app_id: String,
    pub app_secret: String,
    pub encrypt_key: String,
    pub verification_token: String,
    pub allow_from: Vec<String>,
    pub react_emoji: String,
    pub done_emoji: Option<String>,
    pub tool_hint_prefix: String,
    pub group_policy: GroupPolicy,
    pub reply_to_message: bool,
    pub streaming: bool,
    pub domain: FeishuDomain,
    pub topic_isolation: bool,
}

impl Default for FeishuConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            app_id: String::new(),
            app_secret: String::new(),
            encrypt_key: String::new(),
            verification_token: String::new(),
            allow_from: Vec::new(),
            react_emoji: "THUMBSUP".to_string(),
            done_emoji: None,
            tool_hint_prefix: "\u{1f527}".to_string(),
            group_policy: GroupPolicy::Mention,
            reply_to_message: false,
            streaming: true,
            domain: FeishuDomain::Feishu,
            topic_isolation: true,
        }
    }
}

/// Group policy for responding in group chats.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum GroupPolicy {
    Open,
    #[default]
    Mention,
}

/// Feishu/Lark domain selection.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FeishuDomain {
    #[default]
    Feishu,
    Lark,
}

// ---------------------------------------------------------------------------
// Internal types (mirrors Python `_FeishuStreamBuf`)
// ---------------------------------------------------------------------------

#[expect(
    dead_code,
    reason = "reserved for CardKit streaming once the Lark SDK placeholder is connected"
)]
const STREAM_ELEMENT_ID: &str = "streaming_md";

/// Per-chat streaming accumulator using CardKit streaming API.
#[derive(Debug, Default, Clone)]
struct FeishuStreamBuf {
    text: String,
    card_id: Option<String>,
    sequence: u64,
    last_edit: Option<Instant>,
}

// ---------------------------------------------------------------------------
// Lark client placeholder (TODO: replace with actual SDK integration)
// ---------------------------------------------------------------------------

/// Placeholder for the Lark SDK client.
/// TODO: Replace with actual lark-oapi Rust SDK when available.
struct LarkClient {
    // TODO: Actual Lark SDK client
    _app_id: String,
    _app_secret: String,
    _domain: String,
}

impl LarkClient {
    fn new(app_id: &str, app_secret: &str, domain: &str) -> Self {
        Self {
            _app_id: app_id.to_string(),
            _app_secret: app_secret.to_string(),
            _domain: domain.to_string(),
        }
    }
}

/// Placeholder for WebSocket client.
/// TODO: Replace with actual lark-oapi WS client.
struct LarkWsClient {
    // TODO: Actual WS client
}

impl LarkWsClient {
    fn new(
        _app_id: &str,
        _app_secret: &str,
        _domain: &str,
        _encrypt_key: &str,
        _verification_token: &str,
    ) -> Self {
        Self {}
    }
}

// ---------------------------------------------------------------------------
// FeishuChannel (main channel implementation)
// ---------------------------------------------------------------------------

/// Internal state for FeishuChannel, wrapped in Arc for shared ownership.
struct FeishuChannelInner {
    config: FeishuConfig,
    bus: bus::MessageBus,
    client: Mutex<Option<Arc<LarkClient>>>,
    ws_client: Mutex<Option<LarkWsClient>>,
    running: AtomicBool,
    #[expect(
        dead_code,
        reason = "used by the staged inbound handler once the Lark SDK placeholder is connected"
    )]
    processed_message_ids: Mutex<VecDeque<String>>,
    stream_bufs: Mutex<HashMap<String, FeishuStreamBuf>>,
    bot_open_id: Mutex<Option<String>>,
    reaction_ids: Mutex<HashMap<String, String>>,
    transcription_settings: Mutex<TranscriptionSettings>,
}

impl FeishuChannelInner {
    /// Add a reaction emoji to a message (sync placeholder).
    /// TODO: Implement with actual Lark SDK.
    fn _add_reaction_sync(&self, _message_id: &str, _emoji_type: &str) -> Option<String> {
        // TODO: Actual SDK call
        debug!(
            "add_reaction: {} {} (placeholder)",
            _message_id, _emoji_type
        );
        None
    }

    /// Remove a reaction emoji from a message (sync placeholder).
    fn _remove_reaction_sync(&self, _message_id: &str, _reaction_id: &str) {
        // TODO: Actual SDK call
        debug!(
            "remove_reaction: {} {} (placeholder)",
            _message_id, _reaction_id
        );
    }

    /// Download an image from Feishu message.
    /// TODO: Implement with actual Lark SDK.
    fn _download_image_sync(
        &self,
        _message_id: &str,
        _image_key: &str,
    ) -> (Option<Vec<u8>>, Option<String>) {
        warn!("_download_image_sync: not implemented (Lark SDK placeholder)");
        (None, None)
    }

    /// Download a file/audio/media from a Feishu message.
    /// TODO: Implement with actual Lark SDK.
    fn _download_file_sync(
        &self,
        _message_id: &str,
        _file_key: &str,
        _resource_type: &str,
    ) -> (Option<Vec<u8>>, Option<String>) {
        warn!("_download_file_sync: not implemented (Lark SDK placeholder)");
        (None, None)
    }

    /// Fetch the text content of a Feishu message by ID (sync placeholder).
    fn _get_message_content_sync(&self, _message_id: &str) -> Option<String> {
        // TODO: Actual SDK call
        debug!("_get_message_content_sync: not implemented (Lark SDK placeholder)");
        None
    }

    /// Reply to an existing Feishu message (sync placeholder).
    fn _reply_message_sync(
        &self,
        _parent_message_id: &str,
        _msg_type: &str,
        _content: &str,
        _reply_in_thread: bool,
    ) -> bool {
        // TODO: Actual SDK call
        debug!(
            "_reply_message_sync: {} {} (placeholder)",
            _parent_message_id, _msg_type
        );
        false
    }

    /// Send a single message (sync placeholder).
    fn _send_message_sync(
        &self,
        _receive_id_type: &str,
        _receive_id: &str,
        _msg_type: &str,
        _content: &str,
    ) -> Option<String> {
        // TODO: Actual SDK call
        debug!(
            "_send_message_sync: {} {} (placeholder)",
            _receive_id, _msg_type
        );
        None
    }

    /// Upload an image to Feishu and return the image_key.
    /// TODO: Implement with actual Lark SDK.
    fn _upload_image_sync(&self, _file_path: &str) -> Option<String> {
        warn!("_upload_image_sync: not implemented (Lark SDK placeholder)");
        None
    }

    /// Upload a file to Feishu and return the file_key.
    /// TODO: Implement with actual Lark SDK.
    fn _upload_file_sync(&self, _file_path: &str) -> Option<String> {
        warn!("_upload_file_sync: not implemented (Lark SDK placeholder)");
        None
    }

    /// Create a CardKit streaming card (sync placeholder).
    fn _create_streaming_card_sync(
        &self,
        _receive_id_type: &str,
        _chat_id: &str,
        _reply_message_id: Option<&str>,
        _reply_in_thread: bool,
    ) -> Option<String> {
        // TODO: Actual SDK call
        warn!("_create_streaming_card_sync: not implemented (Lark SDK placeholder)");
        None
    }

    /// Stream-update the markdown element on a CardKit card (sync placeholder).
    fn _stream_update_text_sync(&self, _card_id: &str, _content: &str, _sequence: u64) -> bool {
        // TODO: Actual SDK call
        debug!(
            "_stream_update_text_sync: {} seq={} (placeholder)",
            _card_id, _sequence
        );
        false
    }

    /// Turn off CardKit streaming_mode (sync placeholder).
    fn _close_streaming_mode_sync(&self, _card_id: &str, _sequence: u64) -> bool {
        // TODO: Actual SDK call
        debug!(
            "_close_streaming_mode_sync: {} seq={} (placeholder)",
            _card_id, _sequence
        );
        false
    }

    /// Return whether a group reply should create a Feishu thread/topic.
    fn _should_use_reply_in_thread(&self, metadata: &HashMap<String, serde_json::Value>) -> bool {
        let chat_type = metadata
            .get("chat_type")
            .and_then(|v| v.as_str())
            .unwrap_or("group");
        chat_type == "group" && self.config.reply_to_message
    }

    /// Return the message_id that should receive a Reply API response.
    fn _thread_reply_target(
        &self,
        metadata: &HashMap<String, serde_json::Value>,
    ) -> Option<String> {
        let chat_type = metadata
            .get("chat_type")
            .and_then(|v| v.as_str())
            .unwrap_or("group");
        if chat_type != "group" {
            return None;
        }
        let message_id = metadata.get("message_id").and_then(|v| v.as_str())?;
        let has_thread = metadata
            .get("thread_id")
            .map(|v| v.as_str().map(|s| !s.is_empty()).unwrap_or(false))
            .unwrap_or(false);
        if has_thread || self.config.reply_to_message {
            return Some(message_id.to_string());
        }
        None
    }
}

/// Feishu/Lark channel using WebSocket long connection.
///
/// Rust port of Python `FeishuChannel`.
pub struct FeishuChannel {
    inner: Arc<FeishuChannelInner>,
}

impl FeishuChannel {
    /// Create a new Feishu channel with the given configuration.
    pub fn new(config: FeishuConfig, bus: bus::MessageBus) -> Self {
        Self {
            inner: Arc::new(FeishuChannelInner {
                config,
                bus,
                client: Mutex::new(None),
                ws_client: Mutex::new(None),
                running: AtomicBool::new(false),
                processed_message_ids: Mutex::new(VecDeque::new()),
                stream_bufs: Mutex::new(HashMap::new()),
                bot_open_id: Mutex::new(None),
                reaction_ids: Mutex::new(HashMap::new()),
                transcription_settings: Mutex::new(TranscriptionSettings::default()),
            }),
        }
    }

    /// Set transcription settings.
    pub async fn set_transcription_settings(&self, settings: TranscriptionSettings) {
        let mut guard = self.inner.transcription_settings.lock().await;
        *guard = settings;
    }

    /// Check if a sender is allowed.
    #[expect(
        dead_code,
        reason = "used by the staged inbound handler once the Lark SDK placeholder is connected"
    )]
    fn is_allowed(&self, sender_id: &str) -> bool {
        crate::base::is_allowed(
            FeishuChannel::name(),
            &self.inner.config.allow_from,
            sender_id,
        )
    }

    /// Fetch the bot's own open_id via GET /open-apis/bot/v3/info.
    /// TODO: Implement with actual Lark SDK.
    async fn fetch_bot_open_id(&self) -> Option<String> {
        // TODO: Actual SDK call
        warn!("fetch_bot_open_id: not implemented (Lark SDK placeholder)");
        None
    }

    /// Resolve mentions: replace @_user_n placeholders with actual user info.
    fn _resolve_mentions(text: &str, mentions: &[FeishuMention]) -> String {
        if mentions.is_empty() || text.is_empty() {
            return text.to_string();
        }

        let mut result = text.to_string();
        for mention in mentions {
            let key = mention.key.as_ref();
            if key.is_none() || !result.contains(key.unwrap()) {
                continue;
            }
            let key = key.unwrap();

            let user_id_obj = mention.id.as_ref();
            if user_id_obj.is_none() {
                continue;
            }
            let uid = user_id_obj.unwrap();

            let open_id = uid.open_id.as_deref().unwrap_or("");
            let user_id = uid.user_id.as_deref().unwrap_or("");
            let name = mention.name.as_deref().unwrap_or(key);

            let replacement = if !open_id.is_empty() && !user_id.is_empty() {
                format!("@{} ({}, user id: {})", name, open_id, user_id)
            } else if !open_id.is_empty() {
                format!("@{} ({})", name, open_id)
            } else {
                format!("@{}", name)
            };

            result = result.replace(key, &replacement);
        }

        result
    }

    /// Check if the bot is @mentioned in the message.
    fn _is_bot_mentioned(&self, message: &FeishuMessage) -> bool {
        if message.content.contains("@_all") {
            return true;
        }

        for mention in &message.mentions {
            let mid = mention.id.as_ref();
            if mid.is_none() {
                continue;
            }
            let mention_open_id = mid.unwrap().open_id.as_deref().unwrap_or("");

            let bot_open_id = self.inner.bot_open_id.blocking_lock();
            if let Some(ref bot_id) = *bot_open_id {
                if mention_open_id == bot_id.as_str() {
                    return true;
                }
            } else {
                // Fallback heuristic
                let has_user_id = mid
                    .unwrap()
                    .user_id
                    .as_ref()
                    .map(|s| !s.is_empty())
                    .unwrap_or(false);
                if !has_user_id && mention_open_id.starts_with("ou_") {
                    return true;
                }
            }
        }
        false
    }

    /// Allow group messages when policy is open or bot is @mentioned.
    fn _is_group_message_for_bot(&self, message: &FeishuMessage) -> bool {
        if self.inner.config.group_policy == GroupPolicy::Open {
            return true;
        }
        self._is_bot_mentioned(message)
    }

    /// Add a reaction emoji to a message.
    async fn _add_reaction(&self, message_id: &str, emoji_type: &str) -> Option<String> {
        let client = { self.inner.client.lock().await.clone() };
        client.as_ref()?;
        let message_id = message_id.to_string();
        let emoji_type = emoji_type.to_string();
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner._add_reaction_sync(&message_id, &emoji_type))
            .await
            .ok()
            .flatten()
    }

    /// Remove a reaction emoji from a message.
    async fn _remove_reaction(&self, message_id: &str, reaction_id: &str) {
        let client = { self.inner.client.lock().await.clone() };
        if client.is_none() || reaction_id.is_empty() {
            return;
        }
        let message_id = message_id.to_string();
        let reaction_id = reaction_id.to_string();
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            inner._remove_reaction_sync(&message_id, &reaction_id);
        })
        .await
        .ok();
    }

    /// Scope streaming buffers to the inbound message when available.
    fn _stream_key(chat_id: &str, metadata: &HashMap<String, serde_json::Value>) -> String {
        metadata
            .get("message_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| chat_id.to_string())
    }

    // Regex patterns (compiled once via lazy_static equivalent)
    fn table_re() -> &'static Regex {
        use std::sync::OnceLock;
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| {
            Regex::new(
                r"((?:^[ \t]*\|.+\|[ \t]*\n)(?:^[ \t]*\|[-:\s|]+\|[ \t]*\n)(?:^[ \t]*\|.+\|[ \t]*\n?)+)",
            )
            .unwrap()
        })
    }

    fn heading_re() -> &'static Regex {
        use std::sync::OnceLock;
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new(r"^(#{1,6})\s+(.+)$").unwrap())
    }

    fn code_block_re() -> &'static Regex {
        use std::sync::OnceLock;
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new(r"(```[\s\S]*?```)").unwrap())
    }

    fn md_bold_re() -> &'static Regex {
        use std::sync::OnceLock;
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new(r"\*\*(.+?)\*\*").unwrap())
    }

    fn md_bold_underscore_re() -> &'static Regex {
        use std::sync::OnceLock;
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new(r"__(.+?)__").unwrap())
    }

    fn md_strike_re() -> &'static Regex {
        use std::sync::OnceLock;
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new(r"~~(.+?)~~").unwrap())
    }

    fn complex_md_re() -> &'static Regex {
        use std::sync::OnceLock;
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new(r"```|^\|.+\|.*\n\s*\|[-:\s|]+\||^#{1,6}\s+").unwrap())
    }

    fn simple_non_italic_md_re() -> &'static Regex {
        use std::sync::OnceLock;
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new(r"\*\*.+?\*\*|__.+?__|~~.+?~~").unwrap())
    }

    /// Find paired single-star italic markers without treating `**bold**` as italic.
    ///
    /// Byte offsets are safe to slice at because every marker is the ASCII `*`
    /// byte. An unmatched opener is discarded at each line boundary so a pair
    /// never spans lines.
    fn single_star_italic_ranges(text: &str) -> Vec<(usize, usize)> {
        let bytes = text.as_bytes();
        let mut ranges = Vec::new();
        let mut opener = None;

        for (index, byte) in bytes.iter().copied().enumerate() {
            if matches!(byte, b'\n' | b'\r') {
                opener = None;
                continue;
            }
            if byte != b'*' {
                continue;
            }

            let touches_star = index
                .checked_sub(1)
                .is_some_and(|previous| bytes[previous] == b'*')
                || bytes.get(index + 1) == Some(&b'*');
            if touches_star {
                continue;
            }

            if let Some(start) = opener.take() {
                ranges.push((start, index));
            } else {
                opener = Some(index);
            }
        }

        ranges
    }

    fn strip_single_star_italics(text: &str) -> String {
        let ranges = Self::single_star_italic_ranges(text);
        if ranges.is_empty() {
            return text.to_string();
        }

        let mut stripped = String::with_capacity(text.len() - ranges.len() * 2);
        let mut cursor = 0;
        for (start, end) in ranges {
            stripped.push_str(&text[cursor..start]);
            stripped.push_str(&text[start + 1..end]);
            cursor = end + 1;
        }
        stripped.push_str(&text[cursor..]);
        stripped
    }

    fn md_link_re() -> &'static Regex {
        use std::sync::OnceLock;
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new(r"\[([^\]]+)\]\((https?://[^\)]+)\)").unwrap())
    }

    fn list_re() -> &'static Regex {
        use std::sync::OnceLock;
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new(r"^[\s]*[-*+]\s+").unwrap())
    }

    fn olist_re() -> &'static Regex {
        use std::sync::OnceLock;
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new(r"^[\s]*\d+\.\s+").unwrap())
    }

    const STREAM_EDIT_INTERVAL_SECS: f64 = 0.5;
    const TEXT_MAX_LEN: usize = 200;
    const POST_MAX_LEN: usize = 2000;
    #[expect(
        dead_code,
        reason = "reserved for reply-context truncation in the staged inbound handler"
    )]
    const REPLY_CONTEXT_MAX_LEN: usize = 200;

    /// Strip markdown formatting markers from text for plain display.
    fn _strip_md_formatting(text: &str) -> String {
        let text = Self::md_bold_re().replace_all(text, "$1");
        let text = Self::md_bold_underscore_re().replace_all(&text, "$1");
        let text = Self::strip_single_star_italics(&text);
        let text = Self::md_strike_re().replace_all(&text, "$1");
        text.to_string()
    }

    /// Parse a markdown table into a Feishu table element.
    fn _parse_md_table(table_text: &str) -> Option<serde_json::Value> {
        let lines: Vec<&str> = table_text
            .trim()
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();

        if lines.len() < 3 {
            return None;
        }

        fn split_line(line: &str) -> Vec<String> {
            line.trim_matches('|')
                .split('|')
                .map(|c| c.trim().to_string())
                .collect()
        }

        let headers: Vec<String> = split_line(lines[0])
            .iter()
            .map(|h| FeishuChannel::_strip_md_formatting(h))
            .collect();

        let rows: Vec<Vec<String>> = lines[2..]
            .iter()
            .map(|line| {
                split_line(line)
                    .iter()
                    .map(|c| FeishuChannel::_strip_md_formatting(c))
                    .collect()
            })
            .collect();

        let columns: Vec<serde_json::Value> = headers
            .iter()
            .enumerate()
            .map(|(i, h)| {
                serde_json::json!({
                    "tag": "column",
                    "name": format!("c{}", i),
                    "display_name": h,
                    "width": "auto",
                })
            })
            .collect();

        let row_values: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                let mut obj = serde_json::Map::new();
                for i in 0..headers.len() {
                    let val = if i < r.len() {
                        r[i].clone()
                    } else {
                        String::new()
                    };
                    obj.insert(format!("c{}", i), serde_json::Value::String(val));
                }
                serde_json::Value::Object(obj)
            })
            .collect();

        Some(serde_json::json!({
            "tag": "table",
            "page_size": rows.len() + 1,
            "columns": columns,
            "rows": row_values,
        }))
    }

    /// Split content by headings, converting headings to div elements.
    fn _split_headings(&self, content: &str) -> Vec<serde_json::Value> {
        let mut protected = content.to_string();
        let mut code_blocks = Vec::new();

        for m in Self::code_block_re().find_iter(content) {
            code_blocks.push(m.as_str().to_string());
            let placeholder = format!("\x00CODE{}\x00", code_blocks.len() - 1);
            protected = protected.replacen(m.as_str(), &placeholder, 1);
        }

        let mut elements = Vec::new();
        let mut last_end = 0;

        for m in Self::heading_re().captures_iter(&protected) {
            let before = protected[last_end..m.get(0).unwrap().start()].trim();
            if !before.is_empty() {
                elements.push(serde_json::json!({
                    "tag": "markdown",
                    "content": before,
                }));
            }
            let text =
                Self::_strip_md_formatting(m.get(2).map(|g| g.as_str()).unwrap_or("").trim());
            let display_text = if !text.is_empty() {
                format!("**{}**", text)
            } else {
                String::new()
            };
            elements.push(serde_json::json!({
                "tag": "div",
                "text": {
                    "tag": "lark_md",
                    "content": display_text,
                },
            }));
            last_end = m.get(0).unwrap().end();
        }

        let remaining = protected[last_end..].trim();
        if !remaining.is_empty() {
            elements.push(serde_json::json!({
                "tag": "markdown",
                "content": remaining,
            }));
        }

        for (i, cb) in code_blocks.iter().enumerate() {
            let placeholder = format!("\x00CODE{}\x00", i);
            for el in &mut elements {
                if let Some(obj) = el.as_object_mut()
                    && obj.get("tag").and_then(|v| v.as_str()) == Some("markdown")
                    && let Some(content_val) = obj.get_mut("content")
                    && let Some(s) = content_val.as_str()
                {
                    let new_val = s.replace(&placeholder, cb);
                    *content_val = serde_json::Value::String(new_val);
                }
            }
        }

        if elements.is_empty() {
            elements.push(serde_json::json!({
                "tag": "markdown",
                "content": content,
            }));
        }

        elements
    }

    /// Split content into div/markdown + table elements for Feishu card.
    fn _build_card_elements(&self, content: &str) -> Vec<serde_json::Value> {
        let mut elements = Vec::new();
        let mut last_end = 0;

        for m in Self::table_re().find_iter(content) {
            let before = &content[last_end..m.start()];
            if !before.trim().is_empty() {
                elements.extend(self._split_headings(before));
            }
            let table = Self::_parse_md_table(m.as_str())
                .unwrap_or(serde_json::json!({ "tag": "markdown", "content": m.as_str() }));
            elements.push(table);
            last_end = m.end();
        }

        let remaining = &content[last_end..];
        if !remaining.trim().is_empty() {
            elements.extend(self._split_headings(remaining));
        }

        if elements.is_empty() {
            elements.push(serde_json::json!({
                "tag": "markdown",
                "content": content,
            }));
        }

        elements
    }

    /// Split card elements into groups with at most max_tables table elements each.
    fn _split_elements_by_table_limit(
        elements: &[serde_json::Value],
        max_tables: usize,
    ) -> Vec<Vec<serde_json::Value>> {
        if elements.is_empty() {
            return vec![Vec::new()];
        }

        let mut groups = Vec::new();
        let mut current = Vec::new();
        let mut table_count = 0;

        for el in elements {
            if el.get("tag").and_then(|v| v.as_str()) == Some("table") {
                if table_count >= max_tables {
                    if !current.is_empty() {
                        groups.push(std::mem::take(&mut current));
                    }
                    table_count = 0;
                }
                current.push(el.clone());
                table_count += 1;
            } else {
                current.push(el.clone());
            }
        }

        if !current.is_empty() {
            groups.push(current);
        }

        if groups.is_empty() {
            groups.push(Vec::new());
        }

        groups
    }

    /// Determine the optimal Feishu message format for content.
    fn _detect_msg_format(content: &str) -> &'static str {
        let stripped = content.trim();

        if Self::complex_md_re().is_match(stripped) {
            return "interactive";
        }

        if stripped.len() > Self::POST_MAX_LEN {
            return "interactive";
        }

        if Self::simple_non_italic_md_re().is_match(stripped)
            || !Self::single_star_italic_ranges(stripped).is_empty()
        {
            return "interactive";
        }

        if Self::list_re().is_match(stripped) || Self::olist_re().is_match(stripped) {
            return "interactive";
        }

        if Self::md_link_re().is_match(stripped) {
            return "post";
        }

        if stripped.len() <= Self::TEXT_MAX_LEN {
            return "text";
        }

        "post"
    }

    /// Convert markdown content to Feishu post message JSON.
    fn _markdown_to_post(content: &str) -> String {
        let mut paragraphs = Vec::new();

        for line in content.trim().lines() {
            let mut elements = Vec::new();
            let mut last_end = 0;

            for m in Self::md_link_re().captures_iter(line) {
                let before = &line[last_end..m.get(0).unwrap().start()];
                if !before.is_empty() {
                    elements.push(serde_json::json!({ "tag": "text", "text": before }));
                }
                elements.push(serde_json::json!({
                    "tag": "a",
                    "text": m.get(1).map(|g| g.as_str()).unwrap_or(""),
                    "href": m.get(2).map(|g| g.as_str()).unwrap_or(""),
                }));
                last_end = m.get(0).unwrap().end();
            }

            let remaining = &line[last_end..];
            if !remaining.is_empty() {
                elements.push(serde_json::json!({ "tag": "text", "text": remaining }));
            }

            if elements.is_empty() {
                elements.push(serde_json::json!({ "tag": "text", "text": "" }));
            }

            paragraphs.push(elements);
        }

        let post_body = serde_json::json!({
            "zh_cn": {
                "content": paragraphs,
            }
        });

        serde_json::to_string(&post_body).unwrap_or_default()
    }

    /// Return a local-only filename for downloaded Feishu media.
    fn _safe_media_filename(filename: Option<&str>, fallback: &str) -> String {
        let candidate = filename.unwrap_or(fallback);
        // Take basename (handle both / and \ separators)
        let candidate = candidate.replace('\\', "/");
        let candidate_ref = candidate.as_str();
        let candidate = candidate_ref.rsplit('/').next().unwrap_or(candidate_ref);
        let sanitized = safe_filename(candidate);
        if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
            let fb = safe_filename(fallback);
            if fb.is_empty() {
                return uuid::Uuid::new_v4().simple().to_string();
            }
            return fb;
        }
        sanitized
    }

    /// Download media from Feishu and save to local disk.
    async fn _download_and_save_media(
        &self,
        msg_type: &str,
        content_json: &serde_json::Value,
        message_id: Option<&str>,
    ) -> (Option<String>, String) {
        let media_dir = get_media_dir(Some("feishu"));
        let mut data: Option<Vec<u8>> = None;
        let mut filename: Option<String> = None;
        let mut fallback_filename = uuid::Uuid::new_v4().simple().to_string();

        if msg_type == "image" {
            if let Some(image_key) = content_json.get("image_key").and_then(|v| v.as_str())
                && let Some(msg_id) = message_id
            {
                fallback_filename = format!("{}.jpg", &image_key[..image_key.len().min(16)]);
                let (d, f) = tokio::task::spawn_blocking({
                    let inner = Arc::clone(&self.inner);
                    let msg_id = msg_id.to_string();
                    let image_key = image_key.to_string();
                    move || inner._download_image_sync(&msg_id, &image_key)
                })
                .await
                .unwrap_or((None, None));
                data = d;
                filename = f;
                if filename.is_none() {
                    filename = Some(fallback_filename.clone());
                }
            }
        } else if matches!(msg_type, "audio" | "file" | "media") {
            let file_key = content_json.get("file_key").and_then(|v| v.as_str());
            if file_key.is_none() {
                warn!("{} message missing file_key: {:?}", msg_type, content_json);
                return (None, format!("[{}: missing file_key]", msg_type));
            }
            let file_key = file_key.unwrap();
            if message_id.is_none() {
                warn!("{} message missing message_id", msg_type);
                return (None, format!("[{}: missing message_id]", msg_type));
            }

            fallback_filename = file_key[..file_key.len().min(16)].to_string();
            let msg_id = message_id.unwrap().to_string();
            let file_key_for_download = file_key.to_string();
            let msg_type_for_download = msg_type.to_string();
            let (d, f) = tokio::task::spawn_blocking({
                let inner = Arc::clone(&self.inner);
                move || {
                    inner._download_file_sync(
                        &msg_id,
                        &file_key_for_download,
                        &msg_type_for_download,
                    )
                }
            })
            .await
            .unwrap_or((None, None));
            data = d;
            filename = f;

            if data.is_none() {
                warn!("{} download failed: file_key={}", msg_type, file_key);
                return (None, format!("[{}: download failed]", msg_type));
            }

            if filename.is_none() {
                filename = Some(fallback_filename.clone());
            }

            // Feishu voice messages: use .ogg for Whisper compatibility
            if msg_type == "audio"
                && let Some(ref fn_val) = filename
                && !fn_val.ends_with(".opus")
                && !fn_val.ends_with(".ogg")
                && !fn_val.ends_with(".oga")
            {
                filename = Some(format!("{}.ogg", fn_val));
            }
        }

        if let (Some(data), Some(filename)) = (data, filename) {
            let safe_name = Self::_safe_media_filename(Some(&filename), &fallback_filename);
            let file_path = media_dir.join(&safe_name);
            if let Err(e) = std::fs::write(&file_path, &data) {
                warn!("Failed to write media file: {}", e);
                return (None, format!("[{}: write failed]", msg_type));
            }
            let path_str = file_path.to_string_lossy().to_string();
            debug!("Downloaded {} to {}", msg_type, path_str);
            let result_content = format!("[{}: {}]", msg_type, path_str);
            return (Some(path_str), result_content);
        }

        (None, format!("[{}: download failed]", msg_type))
    }

    /// Format a tool hint string with the prefix for each line.
    fn _format_tool_hint_lines(tool_hint: &str) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut buf: Vec<char> = Vec::new();
        let mut depth: u32 = 0;
        let mut in_string = false;
        let mut quote_char = '\0';
        let mut escaped = false;

        let chars: Vec<char> = tool_hint.chars().collect();

        for (i, ch) in chars.iter().enumerate() {
            buf.push(*ch);

            if in_string {
                if escaped {
                    escaped = false;
                } else if *ch == '\\' {
                    escaped = true;
                } else if *ch == quote_char {
                    in_string = false;
                }
                continue;
            }

            if *ch == '"' || *ch == '\'' {
                in_string = true;
                quote_char = *ch;
                continue;
            }

            if *ch == '(' {
                depth += 1;
                continue;
            }

            if *ch == ')' && depth > 0 {
                depth -= 1;
                continue;
            }

            if *ch == ',' && depth == 0 {
                let next_char = chars.get(i + 1).copied().unwrap_or('\0');
                if next_char == ' ' {
                    let s: String = buf.iter().collect();
                    parts.push(s.trim_end().to_string());
                    buf.clear();
                }
            }
        }

        if !buf.is_empty() {
            let s: String = buf.iter().collect();
            parts.push(s.trim().to_string());
        }

        parts
            .iter()
            .filter(|p| !p.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Format a tool hint delta with the configured prefix.
    fn _format_tool_hint_delta(&self, tool_hint: &str) -> String {
        let lines = Self::_format_tool_hint_lines(tool_hint);
        lines
            .lines()
            .filter(|ln| !ln.trim().is_empty())
            .map(|ln| format!("{} {}", self.inner.config.tool_hint_prefix, ln))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[async_trait]
impl Channel for FeishuChannel {
    fn name() -> &'static str {
        "feishu"
    }

    fn display_name() -> &'static str {
        "Feishu"
    }

    fn bus(&self) -> &bus::MessageBus {
        &self.inner.bus
    }

    fn is_running(&self) -> bool {
        self.inner.running.load(Ordering::SeqCst)
    }

    fn streaming_enabled(&self) -> bool {
        self.inner.config.streaming
    }

    fn supports_streaming(&self) -> bool {
        self.inner.config.streaming
    }

    async fn start(self: Arc<Self>) -> ChannelResult<()> {
        let inner = &self.inner;

        if !FEISHU_AVAILABLE {
            error!("SDK not installed. Run: pip install lark-oapi");
            return Err(ChannelError::Permanent(
                "Feishu SDK (lark-oapi) is not installed".to_string(),
            ));
        }

        if inner.config.app_id.is_empty() || inner.config.app_secret.is_empty() {
            error!("app_id and app_secret not configured");
            return Err(ChannelError::Config(
                "Feishu app_id and app_secret are required".to_string(),
            ));
        }

        // Domain selection
        let domain = if inner.config.domain == FeishuDomain::Lark {
            "lark"
        } else {
            "feishu"
        };

        inner.running.store(true, Ordering::SeqCst);

        // Create Lark client for sending messages
        {
            let client = Arc::new(LarkClient::new(
                &inner.config.app_id,
                &inner.config.app_secret,
                domain,
            ));
            *inner.client.lock().await = Some(client);
        }

        // Create WebSocket client for long connection
        {
            let ws = LarkWsClient::new(
                &inner.config.app_id,
                &inner.config.app_secret,
                domain,
                &inner.config.encrypt_key,
                &inner.config.verification_token,
            );
            *inner.ws_client.lock().await = Some(ws);
        }

        // Fetch bot's own open_id
        let bot_open_id = self.fetch_bot_open_id().await;
        if let Some(ref id) = bot_open_id {
            info!("bot open_id: {}", id);
        } else {
            warn!("Could not fetch bot open_id; @mention matching may be inaccurate");
        }
        *inner.bot_open_id.lock().await = bot_open_id;

        info!("bot started with WebSocket long connection");
        info!("No public IP required - using WebSocket to receive events");

        // Keep running until stopped
        while inner.running.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        Ok(())
    }

    async fn stop(self: Arc<Self>) -> ChannelResult<()> {
        self.inner.running.store(false, Ordering::SeqCst);
        info!("Feishu channel stopped");
        Ok(())
    }

    async fn send(self: Arc<Self>, msg: OutboundMessage) -> ChannelResult<()> {
        let client = { self.inner.client.lock().await.clone() };
        if client.is_none() {
            warn!("client not initialized");
            return Ok(());
        }

        let receive_id_type = if msg.chat_id.starts_with("oc_") {
            "chat_id"
        } else {
            "open_id"
        };

        let inner = Arc::clone(&self.inner);

        // Handle tool hint messages
        if msg.metadata.contains_key("_tool_hint") {
            let hint = msg.content.trim();
            if hint.is_empty() {
                return Ok(());
            }

            let stream_key = FeishuChannel::_stream_key(&msg.chat_id, &msg.metadata);
            let buf = {
                let bufs = inner.stream_bufs.lock().await;
                bufs.get(&stream_key).cloned()
            };

            if let Some(ref buf) = buf
                && buf.card_id.is_some()
            {
                // Delegate to send_delta
                let delta = format!("\n\n{}\n\n", self._format_tool_hint_delta(hint));
                let meta_clone: serde_json::Map<String, serde_json::Value> = msg
                    .metadata
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                return self.send_delta(msg.chat_id, delta, meta_clone).await;
            }

            // No active streaming card — send as a regular interactive card
            let card_content = serde_json::json!({
                "config": { "wide_screen_mode": true },
                "elements": [{
                    "tag": "markdown",
                    "content": self._format_tool_hint_delta(hint),
                }],
            });
            let card_str = serde_json::to_string(&card_content).unwrap_or_default();

            let th_msg_id = self.inner._thread_reply_target(&msg.metadata);
            if let Some(ref target_id) = th_msg_id {
                let target_id = target_id.clone();
                let reply_in_thread = self.inner._should_use_reply_in_thread(&msg.metadata);
                tokio::task::spawn_blocking(move || {
                    inner._reply_message_sync(
                        &target_id,
                        "interactive",
                        &card_str,
                        reply_in_thread,
                    );
                })
                .await
                .ok();
            } else {
                let chat_id = msg.chat_id.clone();
                tokio::task::spawn_blocking(move || {
                    inner._send_message_sync(receive_id_type, &chat_id, "interactive", &card_str);
                })
                .await
                .ok();
            }
            return Ok(());
        }

        // Determine reply target
        let msg_id = msg
            .metadata
            .get("message_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let has_thread_id = msg
            .metadata
            .get("thread_id")
            .map(|v| v.as_str().map(|s| !s.is_empty()).unwrap_or(false))
            .unwrap_or(false);

        let reply_message_id = if (self.inner.config.reply_to_message
            && !msg.metadata.contains_key("_progress"))
            || has_thread_id
        {
            msg_id
        } else {
            None
        };

        // Upload and send media
        for file_path in &msg.media {
            if !std::path::Path::new(file_path).is_file() {
                warn!("Media file not found: {}", file_path);
                continue;
            }

            let ext = file_path
                .rsplit('.')
                .next()
                .map(|e| format!(".{}", e))
                .unwrap_or_default()
                .to_lowercase();

            let image_exts: HashSet<&str> = [
                ".png", ".jpg", ".jpeg", ".gif", ".bmp", ".webp", ".ico", ".tiff", ".tif",
            ]
            .iter()
            .copied()
            .collect();

            let audio_exts: HashSet<&str> = [".opus"].iter().copied().collect();
            let video_exts: HashSet<&str> = [".mp4", ".mov", ".avi"].iter().copied().collect();

            let key = if image_exts.contains(ext.as_str()) {
                let fp = file_path.clone();
                tokio::task::spawn_blocking({
                    let inner = Arc::clone(&inner);
                    move || inner._upload_image_sync(&fp)
                })
                .await
                .ok()
                .flatten()
            } else {
                let fp = file_path.clone();
                tokio::task::spawn_blocking({
                    let inner = Arc::clone(&inner);
                    move || inner._upload_file_sync(&fp)
                })
                .await
                .ok()
                .flatten()
            };

            if let Some(key) = key {
                let media_type = if audio_exts.contains(ext.as_str()) {
                    "audio"
                } else if video_exts.contains(ext.as_str()) {
                    "media"
                } else {
                    "file"
                };

                let content = serde_json::json!({ "file_key": key });
                // For images, use image_key
                let content = if image_exts.contains(ext.as_str()) {
                    serde_json::json!({ "image_key": key })
                } else {
                    content
                };
                let content_str = serde_json::to_string(&content).unwrap_or_default();

                let reply_id = reply_message_id.clone();
                let reply_in_thread = self.inner._should_use_reply_in_thread(&msg.metadata);
                let chat_id = msg.chat_id.clone();
                let receive_id_type = receive_id_type.to_string();
                let msg_type = media_type.to_string();

                let inner_for_task = Arc::clone(&inner);
                tokio::task::spawn_blocking(move || {
                    let sent = reply_id.as_ref().is_some_and(|rid| {
                        inner_for_task._reply_message_sync(
                            rid,
                            &msg_type,
                            &content_str,
                            reply_in_thread,
                        )
                    });
                    if !sent {
                        inner_for_task._send_message_sync(
                            &receive_id_type,
                            &chat_id,
                            &msg_type,
                            &content_str,
                        );
                    }
                })
                .await
                .ok();
            }
        }

        // Send text content
        let content = msg.content.trim();
        if !content.is_empty() {
            let fmt = FeishuChannel::_detect_msg_format(content);

            match fmt {
                "text" => {
                    let text_body = serde_json::json!({ "text": content });
                    let text_str = serde_json::to_string(&text_body).unwrap_or_default();
                    let reply_id = reply_message_id.clone();
                    let reply_in_thread = self.inner._should_use_reply_in_thread(&msg.metadata);
                    let chat_id = msg.chat_id.clone();
                    let receive_id_type = receive_id_type.to_string();

                    let inner_for_task = Arc::clone(&inner);
                    tokio::task::spawn_blocking(move || {
                        let sent = reply_id.as_ref().is_some_and(|rid| {
                            inner_for_task._reply_message_sync(
                                rid,
                                "text",
                                &text_str,
                                reply_in_thread,
                            )
                        });
                        if !sent {
                            inner_for_task._send_message_sync(
                                &receive_id_type,
                                &chat_id,
                                "text",
                                &text_str,
                            );
                        }
                    })
                    .await
                    .ok();
                }
                "post" => {
                    let post_body = FeishuChannel::_markdown_to_post(content);
                    let reply_id = reply_message_id.clone();
                    let reply_in_thread = self.inner._should_use_reply_in_thread(&msg.metadata);
                    let chat_id = msg.chat_id.clone();
                    let receive_id_type = receive_id_type.to_string();

                    let inner_for_task = Arc::clone(&inner);
                    tokio::task::spawn_blocking(move || {
                        let sent = reply_id.as_ref().is_some_and(|rid| {
                            inner_for_task._reply_message_sync(
                                rid,
                                "post",
                                &post_body,
                                reply_in_thread,
                            )
                        });
                        if !sent {
                            inner_for_task._send_message_sync(
                                &receive_id_type,
                                &chat_id,
                                "post",
                                &post_body,
                            );
                        }
                    })
                    .await
                    .ok();
                }
                _ => {
                    // interactive
                    let elements = self._build_card_elements(content);
                    let chunks = FeishuChannel::_split_elements_by_table_limit(&elements, 1);

                    for chunk in chunks {
                        let card = serde_json::json!({
                            "config": { "wide_screen_mode": true },
                            "elements": chunk,
                        });
                        let card_str = serde_json::to_string(&card).unwrap_or_default();
                        let reply_id = reply_message_id.clone();
                        let reply_in_thread = self.inner._should_use_reply_in_thread(&msg.metadata);
                        let chat_id = msg.chat_id.clone();
                        let receive_id_type = receive_id_type.to_string();

                        let inner_for_task = Arc::clone(&inner);
                        tokio::task::spawn_blocking(move || {
                            let sent = reply_id.as_ref().is_some_and(|rid| {
                                inner_for_task._reply_message_sync(
                                    rid,
                                    "interactive",
                                    &card_str,
                                    reply_in_thread,
                                )
                            });
                            if !sent {
                                inner_for_task._send_message_sync(
                                    &receive_id_type,
                                    &chat_id,
                                    "interactive",
                                    &card_str,
                                );
                            }
                        })
                        .await
                        .ok();
                    }
                }
            }
        }

        Ok(())
    }

    async fn send_delta(
        self: Arc<Self>,
        chat_id: String,
        delta: String,
        metadata: serde_json::Map<String, serde_json::Value>,
    ) -> ChannelResult<()> {
        let client = { self.inner.client.lock().await.clone() };
        if client.is_none() {
            return Ok(());
        }

        let metadata_hm: HashMap<String, serde_json::Value> = metadata
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let stream_key = FeishuChannel::_stream_key(&chat_id, &metadata_hm);
        let rid_type = if chat_id.starts_with("oc_") {
            "chat_id"
        } else {
            "open_id"
        };

        let inner = Arc::clone(&self.inner);

        // Stream end
        if metadata.contains_key("_stream_end") {
            let message_id = metadata
                .get("message_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            // Reaction cleanup
            if let Some(ref mid) = message_id
                && !metadata.contains_key("_resuming")
            {
                let mut reaction_ids = inner.reaction_ids.lock().await;
                if let Some(reaction_id) = reaction_ids.remove(mid) {
                    drop(reaction_ids);
                    self._remove_reaction(mid, &reaction_id).await;
                } else {
                    drop(reaction_ids);
                }

                // Add completion emoji
                if let Some(ref done_emoji) = inner.config.done_emoji {
                    self._add_reaction(mid, done_emoji).await;
                }
            }

            // Finalize streaming buffer
            let mut bufs = inner.stream_bufs.lock().await;
            let buf = bufs.remove(&stream_key);
            drop(bufs);

            if let Some(mut buf) = buf {
                if buf.text.is_empty() {
                    return Ok(());
                }

                if let Some(ref card_id) = buf.card_id {
                    buf.sequence += 1;
                    let card_id_for_update = card_id.clone();
                    let text = buf.text.clone();
                    let seq = buf.sequence;
                    let ok = tokio::task::spawn_blocking({
                        let inner = Arc::clone(&inner);
                        move || inner._stream_update_text_sync(&card_id_for_update, &text, seq)
                    })
                    .await
                    .unwrap_or(false);

                    if ok {
                        buf.sequence += 1;
                        let card_id_for_close = card_id.clone();
                        let seq = buf.sequence;
                        let _ = tokio::task::spawn_blocking({
                            let inner = Arc::clone(&inner);
                            move || inner._close_streaming_mode_sync(&card_id_for_close, seq)
                        })
                        .await;
                        return Ok(());
                    }

                    warn!(
                        "Streaming card {} final update failed, falling back to regular card",
                        card_id
                    );
                }

                // Fallback: send as regular interactive card
                let elements = self._build_card_elements(&buf.text);
                let chunks = FeishuChannel::_split_elements_by_table_limit(&elements, 1);

                let fallback_msg_id = self.inner._thread_reply_target(&metadata_hm);
                for chunk in chunks {
                    let card = serde_json::json!({
                        "config": { "wide_screen_mode": true },
                        "elements": chunk,
                    });
                    let card_str = serde_json::to_string(&card).unwrap_or_default();

                    let fallback_id = fallback_msg_id.clone();
                    let reply_in_thread = self.inner._should_use_reply_in_thread(&metadata_hm);
                    let chat_id = chat_id.clone();
                    let rid_type = rid_type.to_string();
                    let inner_for_task = Arc::clone(&inner);

                    tokio::task::spawn_blocking(move || {
                        if let Some(ref target) = fallback_id {
                            inner_for_task._reply_message_sync(
                                target,
                                "interactive",
                                &card_str,
                                reply_in_thread,
                            );
                        } else {
                            inner_for_task._send_message_sync(
                                &rid_type,
                                &chat_id,
                                "interactive",
                                &card_str,
                            );
                        }
                    })
                    .await
                    .ok();
                }
            }

            return Ok(());
        }

        // Accumulate delta
        let mut bufs = inner.stream_bufs.lock().await;
        let buf = bufs.get_mut(&stream_key);

        if buf.is_none() {
            bufs.insert(stream_key.clone(), FeishuStreamBuf::default());
        }
        let buf = bufs.get_mut(&stream_key).unwrap();
        buf.text.push_str(&delta);

        if buf.text.trim().is_empty() {
            return Ok(());
        }

        let now = Instant::now();

        if buf.card_id.is_none() {
            // Create streaming card
            let use_reply_in_thread = self.inner._should_use_reply_in_thread(&metadata_hm);
            let reply_msg_id = self.inner._thread_reply_target(&metadata_hm);
            let chat_id_clone = chat_id.clone();
            let rid_type = rid_type.to_string();

            let card_id = tokio::task::spawn_blocking({
                let inner = Arc::clone(&inner);
                move || {
                    inner._create_streaming_card_sync(
                        &rid_type,
                        &chat_id_clone,
                        reply_msg_id.as_deref(),
                        use_reply_in_thread,
                    )
                }
            })
            .await
            .ok()
            .flatten();

            if let Some(card_id) = card_id {
                buf.card_id = Some(card_id.clone());
                buf.sequence = 1;

                let text = buf.text.clone();
                let _ = tokio::task::spawn_blocking({
                    let inner = Arc::clone(&inner);
                    move || inner._stream_update_text_sync(&card_id, &text, 1)
                })
                .await;

                buf.last_edit = Some(now);
            }
        } else {
            // Throttle edits
            let should_update = match buf.last_edit {
                Some(last) => {
                    now.duration_since(last).as_secs_f64()
                        >= FeishuChannel::STREAM_EDIT_INTERVAL_SECS
                }
                None => true,
            };

            if should_update {
                buf.sequence += 1;
                let card_id = buf.card_id.clone().unwrap();
                let text = buf.text.clone();
                let seq = buf.sequence;
                let _ = tokio::task::spawn_blocking({
                    let inner = Arc::clone(&inner);
                    move || inner._stream_update_text_sync(&card_id, &text, seq)
                })
                .await;
                buf.last_edit = Some(now);
            }
        }

        Ok(())
    }

    fn default_config() -> serde_json::Map<String, serde_json::Value>
    where
        Self: Sized,
    {
        let config = FeishuConfig::default();
        serde_json::to_value(&config)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Placeholder types for Feishu message event handling
// ---------------------------------------------------------------------------

/// Placeholder for Feishu sender ID object.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "reserved for inbound events once the Lark SDK placeholder is connected"
    )
)]
#[derive(Debug, Clone, Default)]
struct FeishuSenderId {
    open_id: Option<String>,
    user_id: Option<String>,
}

/// Placeholder for Feishu mention object.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "reserved for inbound events once the Lark SDK placeholder is connected"
    )
)]
#[derive(Debug, Clone, Default)]
struct FeishuMention {
    key: Option<String>,
    id: Option<FeishuSenderId>,
    name: Option<String>,
}

/// Placeholder for Feishu message object.
#[expect(
    dead_code,
    reason = "reserved for inbound events once the Lark SDK placeholder is connected"
)]
#[derive(Debug, Clone, Default)]
struct FeishuMessage {
    message_id: String,
    chat_id: String,
    chat_type: String,
    message_type: String,
    content: String,
    mentions: Vec<FeishuMention>,
    parent_id: Option<String>,
    root_id: Option<String>,
    thread_id: Option<String>,
}

/// Placeholder for Feishu sender object.
#[expect(
    dead_code,
    reason = "reserved for inbound events once the Lark SDK placeholder is connected"
)]
#[derive(Debug, Clone, Default)]
struct FeishuSender {
    sender_type: String,
    sender_id: FeishuSenderId,
}

/// Placeholder for Feishu message event.
#[expect(
    dead_code,
    reason = "reserved for inbound events once the Lark SDK placeholder is connected"
)]
#[derive(Debug, Clone, Default)]
struct FeishuMessageEvent {
    sender: FeishuSender,
    message: FeishuMessage,
}

/// Placeholder for Feishu inbound event data.
#[expect(
    dead_code,
    reason = "reserved for inbound events once the Lark SDK placeholder is connected"
)]
#[derive(Debug, Clone, Default)]
struct FeishuEventData {
    event: FeishuMessageEvent,
}

// ---------------------------------------------------------------------------
// Message handler (mirrors Python `_on_message` and `_on_message_sync`)
// ---------------------------------------------------------------------------

impl FeishuChannel {
    /// Sync handler for incoming messages (called from WebSocket thread).
    /// Schedules async handling in the main event loop.
    async fn _on_message(&self, data: FeishuEventData) {
        let event = data.event;
        let message = event.message;
        let sender = event.sender;

        debug!("raw message: {}", message.content);
        debug!("mentions: {:?}", message.mentions);

        let message_id = message.message_id.clone();

        // Skip bot messages
        if sender.sender_type == "bot" {
            return;
        }

        let sender_id = sender
            .sender_id
            .open_id
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let chat_id = message.chat_id.clone();
        let chat_type = message.chat_type.clone();
        let msg_type = message.message_type.clone();

        // Group policy check
        if chat_type == "group" && !self._is_group_message_for_bot(&message) {
            debug!("skipping group message (not mentioned)");
            return;
        }

        // Deduplication check
        {
            let mut ids = self.inner.processed_message_ids.lock().await;
            if ids.iter().any(|id| id == &message_id) {
                return;
            }
            ids.push_back(message_id.clone());
            // Trim cache
            while ids.len() > 1000 {
                ids.pop_front();
            }
        }

        // Permission check
        if !self.is_allowed(&sender_id) {
            if chat_type == "p2p" {
                let sender_id = sender_id.clone();
                let chat_id = sender_id.clone();
                handle_inbound(
                    &self.inner.bus,
                    FeishuChannel::name(),
                    &self.inner.config.allow_from,
                    self.inner.config.streaming,
                    sender_id,
                    chat_id,
                    "",
                    Vec::new(),
                    None,
                    None,
                )
                .await;
            }
            return;
        }

        // Add reaction (non-blocking)
        let react_emoji = self.inner.config.react_emoji.clone();
        let msg_id_clone = message_id.clone();
        let inner_for_reaction = Arc::clone(&self.inner);
        let inner_for_add_reaction = Arc::clone(&self.inner);

        tokio::spawn(async move {
            let client = inner_for_add_reaction.client.lock().await.clone();
            if client.is_none() {
                return;
            }
            let msg_id = msg_id_clone.clone();
            let emoji = react_emoji.clone();
            let reaction_id = tokio::task::spawn_blocking(move || {
                inner_for_add_reaction._add_reaction_sync(&msg_id, &emoji)
            })
            .await
            .ok()
            .flatten();
            if let Some(reaction_id) = reaction_id {
                let mut reaction_ids = inner_for_reaction.reaction_ids.lock().await;
                reaction_ids.insert(msg_id_clone, reaction_id);
                // Trim cache
                if reaction_ids.len() > 500
                    && let Some(first_key) = reaction_ids.keys().next().cloned()
                {
                    reaction_ids.remove(&first_key);
                }
            }
        });

        // Parse content
        let mut content_parts = Vec::new();
        let mut media_paths = Vec::new();

        let content_json: serde_json::Value =
            serde_json::from_str(&message.content).unwrap_or(serde_json::Value::Null);
        let content_json = if content_json.is_null() {
            serde_json::json!({})
        } else {
            content_json
        };

        if msg_type == "text" {
            if let Some(text) = content_json.get("text").and_then(|v| v.as_str())
                && !text.is_empty()
            {
                let text = Self::_resolve_mentions(text, &message.mentions);
                content_parts.push(text);
            }
        } else if msg_type == "post" {
            let (text, image_keys) = _extract_post_content(&content_json);
            if !text.is_empty() {
                content_parts.push(text);
            }
            for img_key in image_keys {
                let (file_path, content_text) = self
                    ._download_and_save_media(
                        "image",
                        &serde_json::json!({ "image_key": img_key }),
                        Some(&message_id),
                    )
                    .await;
                if let Some(fp) = file_path {
                    media_paths.push(fp);
                }
                content_parts.push(content_text);
            }
        } else if matches!(msg_type.as_str(), "image" | "audio" | "file" | "media") {
            let (file_path, content_text) = self
                ._download_and_save_media(&msg_type, &content_json, Some(&message_id))
                .await;
            if let Some(fp) = file_path {
                media_paths.push(fp);
            }

            let mut content_text = content_text;
            if msg_type == "audio" && media_paths.last().is_some() {
                let file_path = media_paths.last().unwrap().clone();
                let transcription = {
                    let ts = self.inner.transcription_settings.lock().await;
                    crate::base::transcribe_audio(FeishuChannel::name(), &ts, &file_path).await
                };
                if !transcription.is_empty() {
                    content_text = format!("[transcription: {}]", transcription);
                }
            }

            content_parts.push(content_text);
        } else if matches!(
            msg_type.as_str(),
            "share_chat"
                | "share_user"
                | "interactive"
                | "share_calendar_event"
                | "system"
                | "merge_forward"
        ) {
            let text = _extract_share_card_content(&content_json, &msg_type);
            if !text.is_empty() {
                content_parts.push(text);
            }
        } else {
            content_parts.push(msg_type_display(&msg_type).to_string());
        }

        // Extract reply context
        let parent_id = message.parent_id.clone();
        let root_id = message.root_id.clone();
        let thread_id = message.thread_id.clone();

        if parent_id.is_some() {
            let client = { self.inner.client.lock().await.clone() };
            if client.is_some() {
                let parent_id = parent_id.clone().unwrap();
                let reply_ctx = tokio::task::spawn_blocking({
                    let inner = Arc::clone(&self.inner);
                    move || inner._get_message_content_sync(&parent_id)
                })
                .await
                .ok()
                .flatten();
                if let Some(ctx) = reply_ctx {
                    content_parts.insert(0, ctx);
                }
            }
        }

        let content = if content_parts.is_empty() {
            String::new()
        } else {
            content_parts.join("\n")
        };

        if content.is_empty() && media_paths.is_empty() {
            return;
        }

        // Build session key
        let session_key = if chat_type == "group" {
            if self.inner.config.topic_isolation {
                let root_or_msg = root_id.clone().unwrap_or_else(|| message_id.clone());
                Some(format!("feishu:{}:{}", chat_id, root_or_msg))
            } else {
                Some(format!("feishu:{}", chat_id))
            }
        } else {
            None
        };

        // Forward to message bus
        let reply_to = if chat_type == "group" {
            chat_id.clone()
        } else {
            sender_id.clone()
        };

        let mut metadata = serde_json::Map::new();
        metadata.insert("message_id".to_string(), serde_json::json!(message_id));
        metadata.insert("chat_type".to_string(), serde_json::json!(chat_type));
        metadata.insert("msg_type".to_string(), serde_json::json!(msg_type));
        if let Some(ref pid) = parent_id {
            metadata.insert("parent_id".to_string(), serde_json::json!(pid));
        }
        if let Some(ref rid) = root_id {
            metadata.insert("root_id".to_string(), serde_json::json!(rid));
        }
        if let Some(ref tid) = thread_id {
            metadata.insert("thread_id".to_string(), serde_json::json!(tid));
        }

        handle_inbound(
            &self.inner.bus,
            FeishuChannel::name(),
            &self.inner.config.allow_from,
            self.inner.config.streaming,
            sender_id,
            reply_to,
            content,
            media_paths,
            Some(metadata),
            session_key,
        )
        .await;
    }

    /// Stub handlers for reaction/read events (mirrors Python no-op handlers).
    fn _on_reaction_created(&self, _data: FeishuEventData) {
        // Ignore reaction events
    }

    fn _on_reaction_deleted(&self, _data: FeishuEventData) {
        // Ignore reaction deleted events
    }

    fn _on_message_read(&self, _data: FeishuEventData) {
        // Ignore read events
    }

    fn _on_bot_p2p_chat_entered(&self, _data: FeishuEventData) {
        debug!("Bot entered p2p chat (user opened chat window)");
    }
}

// ---------------------------------------------------------------------------
// Builder function for registry
// ---------------------------------------------------------------------------

/// Build a FeishuChannel from config value.
pub fn build(
    section: serde_json::Value,
    bus: bus::MessageBus,
    transcription: TranscriptionSettings,
) -> Result<crate::registry::ChannelEntry, String> {
    let config: FeishuConfig =
        serde_json::from_value(section).map_err(|e| format!("invalid feishu config: {}", e))?;

    if !config.enabled {
        return Err("feishu channel not enabled".to_string());
    }

    let channel = FeishuChannel::new(config, bus);

    // Set transcription settings
    let settings = transcription;
    let channel_for_settings = Arc::clone(&channel.inner);
    tokio::spawn(async move {
        let mut guard = channel_for_settings.transcription_settings.lock().await;
        *guard = settings;
    });

    Ok(crate::registry::ChannelEntry {
        name: FeishuChannel::name().to_string(),
        display_name: FeishuChannel::display_name().to_string(),
        channel: Arc::new(channel),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_share_card_content_share_chat() {
        let json = serde_json::json!({ "chat_id": "oc_123" });
        let result = _extract_share_card_content(&json, "share_chat");
        assert_eq!(result, "[shared chat: oc_123]");
    }

    #[test]
    fn test_extract_share_card_content_system() {
        let json = serde_json::json!({});
        let result = _extract_share_card_content(&json, "system");
        assert_eq!(result, "[system message]");
    }

    #[test]
    fn test_extract_share_card_content_unknown() {
        let json = serde_json::json!({});
        let result = _extract_share_card_content(&json, "unknown_type");
        assert_eq!(result, "[unknown_type]");
    }

    #[test]
    fn test_extract_post_content_direct() {
        let json = serde_json::json!({
            "title": "Hello",
            "content": [[
                [{"tag": "text", "text": "Hello "}, {"tag": "text", "text": "World"}]
            ]]
        });
        let (text, imgs) = _extract_post_content(&json);
        assert_eq!(text, "Hello Hello  World");
        assert!(imgs.is_empty());
    }

    #[test]
    fn test_extract_post_content_wrapped() {
        let json = serde_json::json!({
            "post": {
                "zh_cn": {
                    "title": "Test",
                    "content": [[
                        [{"tag": "text", "text": "Content"}]
                    ]]
                }
            }
        });
        let (text, imgs) = _extract_post_content(&json);
        assert!(text.contains("Content"));
        assert!(imgs.is_empty());
    }

    #[test]
    fn test_detect_msg_format_text() {
        assert_eq!(FeishuChannel::_detect_msg_format("hello"), "text");
    }

    #[test]
    fn test_detect_msg_format_interactive_heading() {
        assert_eq!(
            FeishuChannel::_detect_msg_format("# Title\nSome text"),
            "interactive"
        );
    }

    #[test]
    fn test_detect_msg_format_interactive_code() {
        assert_eq!(
            FeishuChannel::_detect_msg_format("```\ncode\n```"),
            "interactive"
        );
    }

    #[test]
    fn test_detect_msg_format_post_with_link() {
        assert_eq!(
            FeishuChannel::_detect_msg_format("Check [this](https://example.com) out"),
            "post"
        );
    }

    #[test]
    fn test_detect_msg_format_interactive_bold() {
        assert_eq!(
            FeishuChannel::_detect_msg_format("**bold text**"),
            "interactive"
        );
    }

    #[test]
    fn test_single_star_italic_detection_and_stripping() {
        assert_eq!(
            FeishuChannel::_detect_msg_format("before *italic* after"),
            "interactive"
        );
        assert_eq!(
            FeishuChannel::_strip_md_formatting("before *italic* after"),
            "before italic after"
        );
    }

    #[test]
    fn test_single_star_italic_scanner_excludes_bold() {
        assert!(FeishuChannel::single_star_italic_ranges("**bold**").is_empty());
        assert_eq!(FeishuChannel::_strip_md_formatting("**bold**"), "bold");
    }

    #[test]
    fn test_single_star_italic_does_not_cross_newline() {
        let text = "*first line\nsecond line*";
        assert!(FeishuChannel::single_star_italic_ranges(text).is_empty());
        assert_eq!(FeishuChannel::_detect_msg_format(text), "text");
        assert_eq!(FeishuChannel::_strip_md_formatting(text), text);
    }

    #[test]
    fn test_detect_msg_format_interactive_long() {
        let long = "a".repeat(2001);
        assert_eq!(FeishuChannel::_detect_msg_format(&long), "interactive");
    }

    #[test]
    fn test_detect_msg_format_interactive_list() {
        assert_eq!(
            FeishuChannel::_detect_msg_format("- item1\n- item2"),
            "interactive"
        );
    }

    #[test]
    fn test_markdown_to_post_simple() {
        let result = FeishuChannel::_markdown_to_post("Hello\nWorld");
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(parsed.get("zh_cn").is_some());
    }

    #[test]
    fn test_markdown_to_post_with_link() {
        let result = FeishuChannel::_markdown_to_post("Visit [example](https://example.com)");
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        let content = parsed["zh_cn"]["content"][0].as_array().unwrap();
        let link_el = content.iter().find(|e| e["tag"] == "a").unwrap();
        assert_eq!(link_el["text"], "example");
        assert_eq!(link_el["href"], "https://example.com");
    }

    #[test]
    fn test_strip_md_formatting() {
        assert_eq!(FeishuChannel::_strip_md_formatting("**bold**"), "bold");
        assert_eq!(FeishuChannel::_strip_md_formatting("__bold__"), "bold");
        assert_eq!(FeishuChannel::_strip_md_formatting("*italic*"), "italic");
        assert_eq!(FeishuChannel::_strip_md_formatting("~~strike~~"), "strike");
    }

    #[test]
    fn test_format_tool_hint_lines() {
        let input = "tool1(arg1), tool2(arg2)";
        let result = FeishuChannel::_format_tool_hint_lines(input);
        assert_eq!(result, "tool1(arg1)\ntool2(arg2)");
    }

    #[test]
    fn test_format_tool_hint_lines_nested() {
        let input = "outer(inner(a, b), c)";
        let result = FeishuChannel::_format_tool_hint_lines(input);
        // Should not split inside nested parens
        assert_eq!(result, "outer(inner(a, b), c)");
    }

    #[test]
    fn test_feishu_config_defaults() {
        let config = FeishuConfig::default();
        assert!(!config.enabled);
        assert!(config.app_id.is_empty());
        assert!(config.app_secret.is_empty());
        assert_eq!(config.react_emoji, "THUMBSUP");
        assert!(config.done_emoji.is_none());
        assert_eq!(config.tool_hint_prefix, "\u{1f527}");
        assert_eq!(config.group_policy, GroupPolicy::Mention);
        assert!(!config.reply_to_message);
        assert!(config.streaming);
        assert_eq!(config.domain, FeishuDomain::Feishu);
        assert!(config.topic_isolation);
    }

    #[test]
    fn test_safe_media_filename() {
        assert_eq!(
            FeishuChannel::_safe_media_filename(Some("test.txt"), "fallback.txt"),
            "test.txt"
        );
        assert_eq!(
            FeishuChannel::_safe_media_filename(None, "fallback.txt"),
            "fallback.txt"
        );
        assert_eq!(
            FeishuChannel::_safe_media_filename(Some("../../../etc/passwd"), "safe.txt"),
            "safe.txt"
        );
    }

    #[test]
    fn test_resolve_mentions_no_mentions() {
        let text = "Hello @_user_1";
        let result = FeishuChannel::_resolve_mentions(text, &[]);
        assert_eq!(result, "Hello @_user_1");
    }

    #[test]
    fn test_resolve_mentions_with_mention() {
        let text = "Hello @_user_1";
        let mentions = vec![FeishuMention {
            key: Some("@_user_1".to_string()),
            id: Some(FeishuSenderId {
                open_id: Some("ou_123".to_string()),
                user_id: Some("12345".to_string()),
            }),
            name: Some("Alice".to_string()),
        }];
        let result = FeishuChannel::_resolve_mentions(text, &mentions);
        assert!(result.contains("@Alice"));
        assert!(result.contains("ou_123"));
    }
}
