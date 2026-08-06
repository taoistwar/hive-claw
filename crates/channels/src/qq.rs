//! QQ channel — Rust port of `nanobot.channels.qq`.
//!
//! The Python original wraps the `qq-botpy` SDK; the Rust version
//! talks directly to the QQ Bot v2 REST + WebSocket Gateway:
//!
//! * REST: <https://api.sgroup.qq.com>
//! * Gateway: discovered via `GET /gateway`, then connected via
//!   `tokio-tungstenite` (see [`crate::qq_gateway`]).
//!
//! Supported flows:
//!
//! * Access-token refresh via `getAppAccessToken`.
//! * WebSocket gateway with Identify / Resume / heartbeat /
//!   reconnect.
//! * Inbound `C2C_MESSAGE_CREATE` / `GROUP_AT_MESSAGE_CREATE` /
//!   `DIRECT_MESSAGE_CREATE` events.
//! * Attachment download (chunked, atomic `.part` rename).
//! * Outbound text / markdown via `post_*_message`.
//! * Outbound media via base64 file upload + msg_type=7.
//! * Optional ack message ("⏳ Processing...") replied to inbound msg_id.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use futures_util::StreamExt;
use futures_util::future::FutureExt;
use log::{debug, error, info, warn};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, RwLock, mpsc};
use tokio::time::sleep;

use bus::{MessageBus, OutboundMessage};
use config::paths::get_media_dir;

use crate::base::{Channel, ChannelError, ChannelResult, TranscriptionSettings, handle_inbound};
use crate::qq_gateway::{self, DEFAULT_INTENTS, GatewayAuth, GatewayEvent};
use crate::registry::ChannelEntry;

// QQ rich media file_type
const QQ_FILE_TYPE_IMAGE: u32 = 1;
const QQ_FILE_TYPE_FILE: u32 = 4;

fn image_exts() -> &'static [&'static str] {
    &[
        ".png", ".jpg", ".jpeg", ".gif", ".bmp", ".webp", ".tif", ".tiff", ".ico", ".svg",
    ]
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QQConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub secret: String,
    #[serde(default, alias = "allow_from")]
    pub allow_from: Vec<String>,
    #[serde(default = "default_msg_format")]
    pub msg_format: String,
    #[serde(default = "default_ack_message")]
    pub ack_message: String,
    #[serde(default)]
    pub media_dir: String,
    #[serde(default = "default_chunk_size")]
    pub download_chunk_size: usize,
    #[serde(default = "default_max_bytes")]
    pub download_max_bytes: usize,
}

fn default_msg_format() -> String {
    "plain".into()
}
fn default_ack_message() -> String {
    "⏳ Processing...".into()
}
fn default_chunk_size() -> usize {
    1024 * 256
}
fn default_max_bytes() -> usize {
    1024 * 1024 * 200
}

impl Default for QQConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            app_id: String::new(),
            secret: String::new(),
            allow_from: Vec::new(),
            msg_format: default_msg_format(),
            ack_message: default_ack_message(),
            media_dir: String::new(),
            download_chunk_size: default_chunk_size(),
            download_max_bytes: default_max_bytes(),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn safe_name_re() -> &'static Regex {
    use once_cell::sync::Lazy;
    static RE: Lazy<Regex> = Lazy::new(|| {
        // Replace anything that isn't word-char, dot, dash, parens or
        // CJK; matches the Python regex faithfully.
        Regex::new(r"[^\w.\-()\[\]（）【】\u4e00-\u9fff]+").unwrap()
    });
    &RE
}

fn sanitize_filename(name: &str) -> String {
    let trimmed = name.trim();
    let basename = std::path::Path::new(trimmed)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    safe_name_re()
        .replace_all(basename, "_")
        .trim_matches(|c: char| c == '.' || c == '_' || c == ' ')
        .to_string()
}

fn is_image_name(name: &str) -> bool {
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| format!(".{}", e.to_lowercase()))
        .unwrap_or_default();
    image_exts().contains(&ext.as_str())
}

fn guess_send_file_type(filename: &str) -> u32 {
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| format!(".{}", e.to_lowercase()))
        .unwrap_or_default();
    if image_exts().contains(&ext.as_str()) {
        return QQ_FILE_TYPE_IMAGE;
    }
    let mime = mime_guess::from_path(filename).first_or_octet_stream();
    if mime.type_() == mime_guess::mime::IMAGE {
        return QQ_FILE_TYPE_IMAGE;
    }
    QQ_FILE_TYPE_FILE
}

// ---------------------------------------------------------------------------
// Channel
// ---------------------------------------------------------------------------

pub struct QQChannel {
    config: QQConfig,
    bus: MessageBus,
    /// Reserved for parity with the Python channel's optional voice
    /// transcription override; currently unused on QQ inbound.
    #[allow(dead_code)]
    transcription: RwLock<TranscriptionSettings>,
    running: AtomicBool,

    media_root: PathBuf,
    http: Mutex<Option<reqwest::Client>>,
    msg_seq: AtomicI64,

    processed_ids: Mutex<VecDeque<String>>,
    chat_type_cache: Mutex<HashMap<String, ChatType>>,

    access_token: RwLock<AccessToken>,
}

#[derive(Debug, Clone, Copy)]
enum ChatType {
    C2c,
    Group,
}

#[derive(Debug, Default, Clone)]
struct AccessToken {
    token: String,
    /// Unix timestamp (seconds) at which the token expires.
    expires_at: u64,
}

impl QQChannel {
    pub fn from_value(
        value: Value,
        bus: MessageBus,
        transcription: TranscriptionSettings,
    ) -> Result<Self, String> {
        let config: QQConfig =
            serde_json::from_value(value).map_err(|e| format!("invalid qq config: {e}"))?;
        let media_root = init_media_root(&config);
        Ok(Self {
            config,
            bus,
            transcription: RwLock::new(transcription),
            running: AtomicBool::new(false),
            media_root,
            http: Mutex::new(None),
            msg_seq: AtomicI64::new(1),
            processed_ids: Mutex::new(VecDeque::with_capacity(1000)),
            chat_type_cache: Mutex::new(HashMap::new()),
            access_token: RwLock::new(AccessToken::default()),
        })
    }

    async fn ensure_http(&self) -> Result<reqwest::Client, ChannelError> {
        let mut g = self.http.lock().await;
        if g.is_none() {
            let c = reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .connect_timeout(Duration::from_secs(30))
                .build()
                .map_err(ChannelError::Http)?;
            *g = Some(c);
        }
        Ok(g.as_ref().unwrap().clone())
    }

    // ------------------------------------------------------------------
    // Auth — fetch access_token from QQ Bot Open Platform
    // ------------------------------------------------------------------

    async fn refresh_access_token(&self) -> Result<String, ChannelError> {
        let client = self.ensure_http().await?;
        let body = json!({
            "appId": self.config.app_id,
            "clientSecret": self.config.secret,
        });
        let resp = client
            .post("https://bots.qq.com/app/getAppAccessToken")
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        let value: Value = resp.json().await?;
        let token = value
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ChannelError::Other(format!("no access_token in response: {value}")))?
            .to_string();
        let expires_in: u64 = value
            .get("expires_in")
            .and_then(|v| match v {
                Value::String(s) => s.parse::<u64>().ok(),
                Value::Number(n) => n.as_u64(),
                _ => None,
            })
            .unwrap_or(7200);
        let now = unix_secs();
        let mut g = self.access_token.write().await;
        g.token = token.clone();
        // Refresh 60 s before expiry to avoid race conditions.
        g.expires_at = now + expires_in.saturating_sub(60);
        Ok(token)
    }

    async fn get_access_token(&self) -> Result<String, ChannelError> {
        let now = unix_secs();
        {
            let g = self.access_token.read().await;
            if !g.token.is_empty() && now < g.expires_at {
                return Ok(g.token.clone());
            }
        }
        self.refresh_access_token().await
    }

    fn auth_headers(&self, token: &str) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(
            "Authorization",
            format!("QQBot {token}").parse().expect("valid header"),
        );
        h.insert(
            "X-Union-Appid",
            self.config.app_id.parse().expect("valid header"),
        );
        h
    }

    // ------------------------------------------------------------------
    // Outbound (send)
    // ------------------------------------------------------------------

    fn next_seq(&self) -> i64 {
        self.msg_seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    async fn post_message(
        &self,
        chat_id: &str,
        is_group: bool,
        body: Value,
    ) -> Result<Value, ChannelError> {
        let token = self.get_access_token().await?;
        let url = if is_group {
            format!("https://api.sgroup.qq.com/v2/groups/{chat_id}/messages")
        } else {
            format!("https://api.sgroup.qq.com/v2/users/{chat_id}/messages")
        };
        let client = self.ensure_http().await?;
        let resp = client
            .post(&url)
            .headers(self.auth_headers(&token))
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json::<Value>().await?)
    }

    async fn send_text_only(
        &self,
        chat_id: &str,
        is_group: bool,
        msg_id: Option<&str>,
        content: &str,
    ) -> Result<(), ChannelError> {
        let use_md = self.config.msg_format == "markdown";
        let mut payload = json!({
            "msg_type": if use_md { 2 } else { 0 },
            "msg_seq": self.next_seq(),
        });
        if let Some(id) = msg_id {
            payload["msg_id"] = Value::String(id.into());
        }
        if use_md {
            payload["markdown"] = json!({ "content": content });
        } else {
            payload["content"] = Value::String(content.into());
        }
        let _ = self.post_message(chat_id, is_group, payload).await?;
        Ok(())
    }

    async fn post_base64_file(
        &self,
        chat_id: &str,
        is_group: bool,
        file_type: u32,
        file_data_b64: &str,
        file_name: Option<&str>,
        srv_send_msg: bool,
    ) -> Result<Value, ChannelError> {
        let token = self.get_access_token().await?;
        let url = if is_group {
            format!("https://api.sgroup.qq.com/v2/groups/{chat_id}/files")
        } else {
            format!("https://api.sgroup.qq.com/v2/users/{chat_id}/files")
        };
        let mut payload = json!({
            "file_type": file_type,
            "file_data": file_data_b64,
            "srv_send_msg": srv_send_msg,
        });
        if let Some(n) = file_name.filter(|n| file_type != QQ_FILE_TYPE_IMAGE && !n.is_empty()) {
            payload["file_name"] = Value::String(n.into());
        }
        let client = self.ensure_http().await?;
        let resp = client
            .post(&url)
            .headers(self.auth_headers(&token))
            .json(&payload)
            .send()
            .await?
            .error_for_status()?;
        let value: Value = resp.json().await?;
        // Strip extra fields, keep only `file_info` to avoid confusing
        // the QQ client (matches Python behavior).
        if let Some(file_info) = value.get("file_info").cloned() {
            return Ok(json!({ "file_info": file_info }));
        }
        Ok(value)
    }

    async fn read_media_bytes(&self, media_ref: &str) -> Result<(Vec<u8>, String), ChannelError> {
        let media_ref = media_ref.trim();
        if media_ref.is_empty() {
            return Err(ChannelError::Other("empty media reference".into()));
        }

        if !media_ref.starts_with("http://") && !media_ref.starts_with("https://") {
            // Local file or file:// URI.
            let path = if let Some(rest) = media_ref.strip_prefix("file://") {
                let url = url::Url::parse(media_ref).ok();
                if let Some(u) = url {
                    u.to_file_path()
                        .map_err(|_| ChannelError::Other(format!("bad file URI: {media_ref}")))?
                } else {
                    PathBuf::from(rest)
                }
            } else {
                PathBuf::from(expand_tilde(media_ref))
            };
            if !path.is_file() {
                return Err(ChannelError::Other(format!(
                    "outbound media file not found: {}",
                    path.display()
                )));
            }
            let data = fs::read(&path).await?;
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("file")
                .to_string();
            return Ok((data, name));
        }

        // Remote URL — validate via SSRF guard.
        let (ok, err) = security::validate_url_target(media_ref);
        if !ok {
            return Err(ChannelError::Other(format!(
                "outbound media URL validation failed url={media_ref} err={err}"
            )));
        }
        let client = self.ensure_http().await?;
        let resp = client.get(media_ref).send().await?.error_for_status()?;
        let bytes = resp.bytes().await?.to_vec();
        let parsed = url::Url::parse(media_ref).ok();
        let name = parsed
            .as_ref()
            .and_then(|u| {
                u.path_segments()
                    .and_then(|mut s| s.next_back().map(|s| s.to_string()))
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "file.bin".into());
        Ok((bytes, name))
    }

    async fn send_media(
        &self,
        chat_id: &str,
        media_ref: &str,
        msg_id: Option<&str>,
        is_group: bool,
    ) -> Result<bool, ChannelError> {
        let (data, filename) = match self.read_media_bytes(media_ref).await {
            Ok(t) => t,
            Err(ChannelError::Other(_)) => {
                // File-not-found or validation error — non-retryable.
                warn!("QQ outbound media read error ref={media_ref} err=file not found");
                return Ok(false);
            }
            Err(e) => {
                // Network / transport error — propagate for retry.
                return Err(e);
            }
        };
        if data.is_empty() || filename.is_empty() {
            return Ok(false);
        }
        let file_type = guess_send_file_type(&filename);
        let file_data_b64 = B64.encode(&data);
        let media_obj = match self
            .post_base64_file(
                chat_id,
                is_group,
                file_type,
                &file_data_b64,
                Some(&filename),
                false,
            )
            .await
        {
            Ok(v) => v,
            Err(e) if e.is_retryable() => {
                // Network error during upload — propagate for retry.
                warn!("QQ send media network error filename={} err={e}", filename);
                return Err(e);
            }
            Err(e) => {
                // API-level error — return False so send() can fallback to text.
                error!("QQ send media failed filename={} err={e}", filename);
                return Ok(false);
            }
        };

        let mut payload = json!({
            "msg_type": 7,
            "msg_seq": self.next_seq(),
            "media": media_obj,
        });
        if let Some(id) = msg_id {
            payload["msg_id"] = Value::String(id.into());
        }
        match self.post_message(chat_id, is_group, payload).await {
            Ok(_) => {
                info!("QQ media sent: {filename}");
                Ok(true)
            }
            Err(e) if e.is_retryable() => Err(e),
            Err(e) => {
                error!("QQ send media failed filename={} err={e}", filename);
                Ok(false)
            }
        }
    }

    // ------------------------------------------------------------------
    // Inbound (receive) — WebSocket gateway driver
    // ------------------------------------------------------------------

    /// Fetch the WebSocket gateway URL via REST `/gateway`.
    async fn fetch_ws_url(&self) -> Result<String, ChannelError> {
        let token = self.get_access_token().await?;
        let client = self.ensure_http().await?;
        let resp = client
            .get("https://api.sgroup.qq.com/gateway")
            .headers(self.auth_headers(&token))
            .send()
            .await?
            .error_for_status()?;
        let value: Value = resp.json().await?;
        let url = value
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ChannelError::Other(format!("missing url in /gateway response: {value}"))
            })?
            .to_string();
        Ok(url)
    }

    /// Drive the WebSocket gateway in this task. Runs until
    /// `self.running` flips to false.
    async fn run_gateway(self: Arc<Self>) {
        let (tx, mut rx) = mpsc::channel::<GatewayEvent>(128);
        let running = Arc::new(AtomicBool::new(true));

        // Mirror channel-level `running` into the gateway flag.
        let mirror_self = Arc::clone(&self);
        let mirror_flag = Arc::clone(&running);
        let mirror_handle = tokio::spawn(async move {
            while mirror_self.running.load(Ordering::SeqCst) {
                sleep(Duration::from_millis(500)).await;
            }
            mirror_flag.store(false, Ordering::SeqCst);
        });

        // Closures that re-fetch the WS URL and a fresh access token
        // for each (re)connect attempt.
        let url_self = Arc::clone(&self);
        let auth_self = Arc::clone(&self);
        let intents = DEFAULT_INTENTS;

        let url_fn = move || {
            let s = Arc::clone(&url_self);
            async move {
                s.fetch_ws_url()
                    .await
                    .map_err(|e| format!("fetch ws url: {e}"))
            }
            .boxed()
        };
        let auth_fn = move || {
            let s = Arc::clone(&auth_self);
            async move {
                let token = s
                    .get_access_token()
                    .await
                    .map_err(|e| format!("get access token: {e}"))?;
                Ok(GatewayAuth {
                    app_id: s.config.app_id.clone(),
                    access_token: token,
                    intents,
                })
            }
            .boxed()
        };

        // Run the gateway loop in the background while we consume events.
        let gw_running = Arc::clone(&running);
        let gw_tx = tx.clone();
        let gateway_handle = tokio::spawn(async move {
            qq_gateway::run_with_reconnect(url_fn, auth_fn, gw_running, gw_tx).await;
        });

        while let Some(event) = rx.recv().await {
            if !self.running.load(Ordering::SeqCst) {
                break;
            }
            let this = Arc::clone(&self);
            // Each event handler may do non-trivial work (download
            // attachments, send ack); spawn so the gateway pump never
            // stalls.
            tokio::spawn(async move {
                this.handle_gateway_event(event).await;
            });
        }

        running.store(false, Ordering::SeqCst);
        mirror_handle.abort();
        gateway_handle.abort();
        let _ = gateway_handle.await;
    }

    async fn handle_gateway_event(self: Arc<Self>, event: GatewayEvent) {
        match event {
            GatewayEvent::Ready {
                session_id,
                bot_name,
            } => {
                info!("QQ bot ready: name={bot_name} session={session_id}");
            }
            GatewayEvent::C2cMessage(d) => {
                if let Err(e) = self.on_message(d, ChatType::C2c).await {
                    error!("QQ inbound c2c error: {e}");
                }
            }
            GatewayEvent::GroupAtMessage(d) => {
                if let Err(e) = self.on_message(d, ChatType::Group).await {
                    error!("QQ inbound group error: {e}");
                }
            }
            GatewayEvent::DirectMessage(d) => {
                if let Err(e) = self.on_message(d, ChatType::C2c).await {
                    error!("QQ inbound direct error: {e}");
                }
            }
        }
    }

    async fn on_message(
        self: &Arc<Self>,
        data: Value,
        chat_type: ChatType,
    ) -> Result<(), ChannelError> {
        let id = data
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            return Ok(());
        }
        {
            let mut seen = self.processed_ids.lock().await;
            if seen.iter().any(|p| p == &id) {
                return Ok(());
            }
            seen.push_back(id.clone());
            while seen.len() > 1000 {
                seen.pop_front();
            }
        }

        let is_group = matches!(chat_type, ChatType::Group);
        let (chat_id, user_id) = if is_group {
            let chat = data
                .get("group_openid")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let user = data
                .get("author")
                .and_then(|a| a.get("member_openid"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            (chat, user)
        } else {
            let user = data
                .get("author")
                .and_then(|a| {
                    a.get("id")
                        .and_then(|v| v.as_str())
                        .or_else(|| a.get("user_openid").and_then(|v| v.as_str()))
                })
                .unwrap_or("unknown")
                .to_string();
            (user.clone(), user)
        };
        if chat_id.is_empty() {
            return Ok(());
        }

        self.chat_type_cache
            .lock()
            .await
            .insert(chat_id.clone(), chat_type);

        let mut content = data
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        let attachments = data
            .get("attachments")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let (media_paths, recv_lines, att_meta) = self.handle_attachments(&attachments).await;

        if !recv_lines.is_empty() {
            let tag = if media_paths.iter().any(|p| {
                is_image_name(
                    Path::new(p)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(""),
                )
            }) {
                "[Image]"
            } else {
                "[File]"
            };
            let block = format!("Received files:\n{}", recv_lines.join("\n"));
            content = if !content.is_empty() {
                format!("{content}\n\n{block}").trim().to_string()
            } else {
                format!("{tag}\n{block}")
            };
        }

        if content.is_empty() && media_paths.is_empty() {
            return Ok(());
        }

        // Optional ack (best-effort).
        if !self.config.ack_message.is_empty() {
            match self
                .send_text_only(&chat_id, is_group, Some(&id), &self.config.ack_message)
                .await
            {
                Ok(_) => {}
                Err(e) => debug!("QQ ack message failed for chat_id={chat_id}: {e}"),
            }
        }

        let mut metadata = Map::new();
        metadata.insert("message_id".into(), Value::String(id.clone()));
        metadata.insert(
            "attachments".into(),
            Value::Array(att_meta.into_iter().map(Value::Object).collect()),
        );

        handle_inbound(
            &self.bus,
            QQChannel::name(),
            &self.config.allow_from,
            self.supports_streaming(),
            user_id,
            chat_id,
            content,
            media_paths,
            Some(metadata),
            None,
        )
        .await;

        Ok(())
    }

    async fn handle_attachments(
        &self,
        attachments: &[Value],
    ) -> (Vec<String>, Vec<String>, Vec<Map<String, Value>>) {
        let mut media_paths: Vec<String> = Vec::new();
        let mut recv_lines: Vec<String> = Vec::new();
        let mut att_meta: Vec<Map<String, Value>> = Vec::new();

        for att in attachments {
            let url = att
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let filename = att
                .get("filename")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let ctype = att
                .get("content_type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            info!(
                "Downloading file from QQ: {}",
                if !filename.is_empty() {
                    &filename
                } else {
                    &url
                }
            );
            let local = self
                .download_to_media_dir_chunked(&url, &filename, &ctype)
                .await;

            let mut meta = Map::new();
            meta.insert("url".into(), Value::String(url.clone()));
            meta.insert("filename".into(), Value::String(filename.clone()));
            meta.insert("content_type".into(), Value::String(ctype.clone()));
            meta.insert(
                "saved_path".into(),
                local.clone().map(Value::String).unwrap_or(Value::Null),
            );
            att_meta.push(meta);

            match local {
                Some(path) => {
                    let shown = if !filename.is_empty() {
                        filename
                    } else {
                        Path::new(&path)
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or(&path)
                            .to_string()
                    };
                    recv_lines.push(format!("- {shown}\n  saved: {path}"));
                    media_paths.push(path);
                }
                None => {
                    let shown = if !filename.is_empty() { filename } else { url };
                    recv_lines.push(format!("- {shown}\n  saved: [download failed]"));
                }
            }
        }

        (media_paths, recv_lines, att_meta)
    }

    async fn download_to_media_dir_chunked(
        &self,
        url: &str,
        filename_hint: &str,
        ctype_hint: &str,
    ) -> Option<String> {
        // Protocol-relative URLs ("//multimedia.nt.qq.com/...").
        let normalized: String = if url.starts_with("//") {
            format!("https:{url}")
        } else {
            url.to_string()
        };
        let result = self
            .download_inner(&normalized, filename_hint, ctype_hint)
            .await;
        match result {
            Ok(path) => Some(path),
            Err(e) => {
                error!("QQ download error: {e}");
                None
            }
        }
    }

    async fn download_inner(
        &self,
        url: &str,
        filename_hint: &str,
        ctype_hint: &str,
    ) -> Result<String, ChannelError> {
        let client = self.ensure_http().await?;
        let resp = client.get(url).send().await?;
        if !resp.status().is_success() {
            return Err(ChannelError::Other(format!(
                "download failed: status={} url={url}",
                resp.status()
            )));
        }

        let parsed_path = url::Url::parse(url)
            .ok()
            .and_then(|u| {
                u.path_segments()
                    .and_then(|mut segs| segs.next_back().map(|s| s.to_string()))
            })
            .unwrap_or_default();
        let url_ext = Path::new(&parsed_path)
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| format!(".{}", e))
            .unwrap_or_default();
        let hint_ext = Path::new(filename_hint)
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| format!(".{}", e))
            .unwrap_or_default();
        let ext = if !url_ext.is_empty() {
            url_ext
        } else if !hint_ext.is_empty() {
            hint_ext
        } else {
            let lower = ctype_hint.to_ascii_lowercase();
            if lower.contains("png") {
                ".png".into()
            } else if lower.contains("jpeg") || lower.contains("jpg") {
                ".jpg".into()
            } else if lower.contains("gif") {
                ".gif".into()
            } else if lower.contains("webp") {
                ".webp".into()
            } else if lower.contains("pdf") {
                ".pdf".into()
            } else {
                ".bin".into()
            }
        };

        let safe = sanitize_filename(filename_hint);
        let ts = unix_millis();
        let filename = if !safe.is_empty() {
            if Path::new(&safe).extension().is_some() {
                safe
            } else {
                format!("{safe}{ext}")
            }
        } else {
            format!("qq_file_{ts}{ext}")
        };

        let mut target = self.media_root.join(&filename);
        if target.exists() {
            let stem = Path::new(&filename)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("file");
            let suffix = Path::new(&filename)
                .extension()
                .and_then(|s| s.to_str())
                .map(|e| format!(".{}", e))
                .unwrap_or_default();
            target = self.media_root.join(format!("{stem}_{ts}{suffix}"));
        }
        let mut tmp_path = target.clone();
        let mut tmp_name = tmp_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        tmp_name.push_str(".part");
        tmp_path.set_file_name(tmp_name);

        if let Some(parent) = tmp_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        let mut f = fs::File::create(&tmp_path).await?;

        let chunk_size = self.config.download_chunk_size.max(1024);
        let max_bytes = self.config.download_max_bytes.max(1024 * 1024);
        let mut downloaded: usize = 0;
        let mut stream = resp.bytes_stream();
        let mut bailed = false;

        while let Some(item) = stream.next().await {
            let bytes = item?;
            if bytes.is_empty() {
                continue;
            }
            // Honour chunk_size by splitting larger frames before write.
            for piece in bytes.chunks(chunk_size) {
                downloaded += piece.len();
                if downloaded > max_bytes {
                    warn!("QQ download exceeded max_bytes={max_bytes} url={url} -> abort");
                    bailed = true;
                    break;
                }
                f.write_all(piece).await?;
            }
            if bailed {
                break;
            }
        }
        f.flush().await?;
        drop(f);

        if bailed {
            let _ = fs::remove_file(&tmp_path).await;
            return Err(ChannelError::Other(format!(
                "download exceeded {max_bytes} bytes"
            )));
        }

        fs::rename(&tmp_path, &target).await?;
        info!("QQ file saved: {}", target.display());
        Ok(target.to_string_lossy().into_owned())
    }
}

#[async_trait]
impl Channel for QQChannel {
    fn name() -> &'static str {
        "qq"
    }
    fn display_name() -> &'static str {
        "QQ"
    }
    fn bus(&self) -> &MessageBus {
        &self.bus
    }
    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn default_config() -> serde_json::Map<String, Value> {
        serde_json::to_value(QQConfig::default())
            .ok()
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    }

    async fn start(self: Arc<Self>) -> ChannelResult<()> {
        if self.config.app_id.is_empty() || self.config.secret.is_empty() {
            error!("QQ app_id and secret not configured");
            return Ok(());
        }
        self.running.store(true, Ordering::SeqCst);
        // Eagerly fetch access token so REST sends work immediately.
        match self.refresh_access_token().await {
            Ok(_) => info!("QQ bot authenticated (REST surface ready)"),
            Err(e) => {
                error!("QQ access_token fetch failed: {e}");
                self.running.store(false, Ordering::SeqCst);
                return Err(e);
            }
        }
        // Drive the WebSocket gateway in this task so `start()` only
        // returns when the channel is asked to stop.
        self.run_gateway().await;
        Ok(())
    }

    async fn stop(self: Arc<Self>) -> ChannelResult<()> {
        self.running.store(false, Ordering::SeqCst);
        *self.http.lock().await = None;
        info!("QQ bot stopped");
        Ok(())
    }

    async fn send(self: Arc<Self>, msg: OutboundMessage) -> ChannelResult<()> {
        if !self.running.load(Ordering::SeqCst) {
            warn!("QQ client not initialized");
            return Ok(());
        }
        let msg_id = msg
            .metadata
            .get("message_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let chat_type = self
            .chat_type_cache
            .lock()
            .await
            .get(&msg.chat_id)
            .copied()
            .unwrap_or(ChatType::C2c);
        let is_group = matches!(chat_type, ChatType::Group);

        for media_ref in msg.media.iter() {
            match self
                .send_media(&msg.chat_id, media_ref, msg_id.as_deref(), is_group)
                .await
            {
                Ok(true) => {}
                Ok(false) => {
                    // API-level or non-network error — fallback to text.
                    let filename = url::Url::parse(media_ref)
                        .ok()
                        .and_then(|u| {
                            u.path_segments()
                                .and_then(|mut s| s.next_back().map(|s| s.to_string()))
                        })
                        .or_else(|| {
                            std::path::Path::new(media_ref)
                                .file_name()
                                .and_then(|s| s.to_str())
                                .map(|s| s.to_string())
                        })
                        .unwrap_or_else(|| "file".into());
                    let _ = self
                        .send_text_only(
                            &msg.chat_id,
                            is_group,
                            msg_id.as_deref(),
                            &format!("[Attachment send failed: {filename}]"),
                        )
                        .await;
                }
                Err(e) if e.is_retryable() => {
                    // Network / transport errors — propagate so
                    // ChannelManager can apply retry policy.
                    return Err(e);
                }
                Err(_) => {
                    // Non-retryable error — fallback to text.
                    let filename = url::Url::parse(media_ref)
                        .ok()
                        .and_then(|u| {
                            u.path_segments()
                                .and_then(|mut s| s.next_back().map(|s| s.to_string()))
                        })
                        .or_else(|| {
                            std::path::Path::new(media_ref)
                                .file_name()
                                .and_then(|s| s.to_str())
                                .map(|s| s.to_string())
                        })
                        .unwrap_or_else(|| "file".into());
                    let _ = self
                        .send_text_only(
                            &msg.chat_id,
                            is_group,
                            msg_id.as_deref(),
                            &format!("[Attachment send failed: {filename}]"),
                        )
                        .await;
                }
            }
        }

        let content = msg.content.trim();
        if !content.is_empty() {
            self.send_text_only(&msg.chat_id, is_group, msg_id.as_deref(), content)
                .await?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn init_media_root(cfg: &QQConfig) -> PathBuf {
    let root = if !cfg.media_dir.is_empty() {
        PathBuf::from(expand_tilde(&cfg.media_dir))
    } else {
        get_media_dir(Some("qq"))
    };
    let _ = std::fs::create_dir_all(&root);
    info!("QQ media directory: {}", root.display());
    root
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn expand_tilde(p: &str) -> String {
    if let Some(expanded) = p.strip_prefix("~/").and_then(|rest| {
        dirs::home_dir().map(|home| home.join(rest).to_string_lossy().into_owned())
    }) {
        return expanded;
    }
    if p == "~" {
        return dirs::home_dir()
            .map(|home| home.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.to_string());
    }
    p.to_string()
}

// ---------------------------------------------------------------------------
// Registry hook
// ---------------------------------------------------------------------------

pub(crate) fn build(
    section: Value,
    bus: MessageBus,
    transcription: TranscriptionSettings,
) -> Result<ChannelEntry, String> {
    let ch = QQChannel::from_value(section, bus, transcription)?;
    let arc: Arc<dyn Channel> = Arc::new(ch);
    Ok(ChannelEntry {
        name: "qq".into(),
        display_name: "QQ".into(),
        channel: arc,
    })
}

// Keep Path re-export for sanitize tests.
#[allow(dead_code)]
fn _path_anchor(_p: &Path) {}
