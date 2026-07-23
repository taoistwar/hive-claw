//! Personal WeChat (微信) channel using HTTP long-poll.
//!
//! Rust port of `nanobot.channels.weixin`. Connects to
//! `ilinkai.weixin.qq.com` to receive and send personal WeChat
//! messages. Authentication is via QR code login that produces a bot
//! token.
//!
//! Protocol reverse-engineered from `@tencent-weixin/openclaw-weixin`
//! v1.0.3.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aes::Aes128;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};
use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use log::{debug, error, info, warn};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::fs;
use tokio::sync::{Mutex, Notify, RwLock};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use uuid::Uuid;

use bus::{MessageBus, OutboundMessage};
use config::paths::{get_media_dir, get_runtime_subdir};
use utils::helpers::split_message;

use crate::base::{
    Channel, ChannelError, ChannelResult, TranscriptionSettings, handle_inbound, transcribe_audio,
};
use crate::registry::ChannelEntry;

// ---------------------------------------------------------------------------
// Protocol constants (from openclaw-weixin types.ts)
// ---------------------------------------------------------------------------

const ITEM_TEXT: u32 = 1;
const ITEM_IMAGE: u32 = 2;
const ITEM_VOICE: u32 = 3;
const ITEM_FILE: u32 = 4;
const ITEM_VIDEO: u32 = 5;

const MESSAGE_TYPE_BOT: u32 = 2;
const MESSAGE_STATE_FINISH: u32 = 2;

const WEIXIN_MAX_MESSAGE_LEN: usize = 4000;
const WEIXIN_CHANNEL_VERSION: &str = "2.1.1";
const ILINK_APP_ID: &str = "bot";

const ERRCODE_SESSION_EXPIRED: i64 = -14;
const SESSION_PAUSE_DURATION_S: u64 = 60 * 60;

const MAX_CONSECUTIVE_FAILURES: u32 = 3;
const BACKOFF_DELAY_S: u64 = 30;
const RETRY_DELAY_S: u64 = 2;
const MAX_QR_REFRESH_COUNT: u32 = 3;
const TYPING_STATUS_TYPING: u32 = 1;
const TYPING_STATUS_CANCEL: u32 = 2;
const TYPING_TICKET_TTL_S: u64 = 24 * 60 * 60;
const TYPING_KEEPALIVE_INTERVAL_S: u64 = 5;
const CONFIG_CACHE_INITIAL_RETRY_S: u64 = 2;
const CONFIG_CACHE_MAX_RETRY_S: u64 = 60 * 60;

const DEFAULT_LONG_POLL_TIMEOUT_S: u64 = 35;

const UPLOAD_MEDIA_IMAGE: u32 = 1;
const UPLOAD_MEDIA_VIDEO: u32 = 2;
const UPLOAD_MEDIA_FILE: u32 = 3;
const UPLOAD_MEDIA_VOICE: u32 = 4;

fn image_exts() -> &'static [&'static str] {
    &[
        ".jpg", ".jpeg", ".png", ".gif", ".bmp", ".webp", ".tiff", ".ico", ".svg",
    ]
}
fn video_exts() -> &'static [&'static str] {
    &[".mp4", ".avi", ".mov", ".mkv", ".webm", ".flv"]
}
fn voice_exts() -> &'static [&'static str] {
    &[
        ".mp3", ".wav", ".amr", ".silk", ".ogg", ".m4a", ".aac", ".flac",
    ]
}

fn build_client_version(version: &str) -> u32 {
    let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let major = parts.next().unwrap_or(0) & 0xFF;
    let minor = parts.next().unwrap_or(0) & 0xFF;
    let patch = parts.next().unwrap_or(0) & 0xFF;
    (major << 16) | (minor << 8) | patch
}

fn base_info() -> Value {
    json!({ "channel_version": WEIXIN_CHANNEL_VERSION })
}

fn has_downloadable_media_locator(media: Option<&Value>) -> bool {
    let Some(media) = media else { return false };
    let Some(obj) = media.as_object() else {
        return false;
    };
    let q = obj
        .get("encrypt_query_param")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let f = obj
        .get("full_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    !q.is_empty() || !f.is_empty()
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeixinConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, alias = "allow_from")]
    pub allow_from: Vec<String>,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default = "default_cdn_base_url")]
    pub cdn_base_url: String,
    #[serde(default)]
    pub route_tag: Option<String>,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub state_dir: String,
    #[serde(default = "default_poll_timeout")]
    pub poll_timeout: u64,
}

fn default_base_url() -> String {
    "https://ilinkai.weixin.qq.com".to_string()
}
fn default_cdn_base_url() -> String {
    "https://novac2c.cdn.weixin.qq.com/c2c".to_string()
}
fn default_poll_timeout() -> u64 {
    DEFAULT_LONG_POLL_TIMEOUT_S
}

impl Default for WeixinConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allow_from: Vec::new(),
            base_url: default_base_url(),
            cdn_base_url: default_cdn_base_url(),
            route_tag: None,
            token: String::new(),
            state_dir: String::new(),
            poll_timeout: DEFAULT_LONG_POLL_TIMEOUT_S,
        }
    }
}

// ---------------------------------------------------------------------------
// State persisted to disk
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct PersistentState {
    #[serde(default)]
    token: String,
    #[serde(default)]
    get_updates_buf: String,
    #[serde(default)]
    context_tokens: HashMap<String, String>,
    #[serde(default)]
    typing_tickets: HashMap<String, TicketEntry>,
    #[serde(default)]
    base_url: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct TicketEntry {
    #[serde(default)]
    ticket: String,
    #[serde(default)]
    ever_succeeded: bool,
    #[serde(default)]
    next_fetch_at: f64,
    #[serde(default)]
    retry_delay_s: f64,
}

// ---------------------------------------------------------------------------
// Channel
// ---------------------------------------------------------------------------

pub struct WeixinChannel {
    config: RwLock<WeixinConfig>,
    bus: MessageBus,
    transcription: RwLock<TranscriptionSettings>,
    running: std::sync::atomic::AtomicBool,

    http: Mutex<Option<reqwest::Client>>,

    inner: Mutex<InnerState>,
    typing: Mutex<HashMap<String, TypingTask>>,
}

struct InnerState {
    state: PersistentState,
    state_dir: Option<PathBuf>,
    processed_ids: VecDeque<String>,
    next_poll_timeout_s: u64,
    session_pause_until: Option<Instant>,
}

struct TypingTask {
    stop: Arc<Notify>,
    handle: JoinHandle<()>,
}

impl WeixinChannel {
    pub fn from_value(
        value: Value,
        bus: MessageBus,
        transcription: TranscriptionSettings,
    ) -> Result<Self, String> {
        let config: WeixinConfig =
            serde_json::from_value(value).map_err(|e| format!("invalid weixin config: {e}"))?;

        Ok(Self {
            config: RwLock::new(config),
            bus,
            transcription: RwLock::new(transcription),
            running: std::sync::atomic::AtomicBool::new(false),
            http: Mutex::new(None),
            inner: Mutex::new(InnerState {
                state: PersistentState::default(),
                state_dir: None,
                processed_ids: VecDeque::with_capacity(1000),
                next_poll_timeout_s: DEFAULT_LONG_POLL_TIMEOUT_S,
                session_pause_until: None,
            }),
            typing: Mutex::new(HashMap::new()),
        })
    }

    fn http_client(&self, total_timeout_s: u64) -> Result<reqwest::Client, ChannelError> {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(total_timeout_s))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(ChannelError::Http)
    }

    async fn ensure_http(&self, total_timeout_s: u64) -> Result<reqwest::Client, ChannelError> {
        let mut g = self.http.lock().await;
        if g.is_none() {
            *g = Some(self.http_client(total_timeout_s)?);
        }
        Ok(g.as_ref().unwrap().clone())
    }

    // ------------------------------------------------------------------
    // State persistence
    // ------------------------------------------------------------------

    async fn get_state_dir(&self) -> PathBuf {
        let mut inner = self.inner.lock().await;
        if let Some(d) = &inner.state_dir {
            return d.clone();
        }
        let cfg_state = self.config.read().await.state_dir.clone();
        let dir = if !cfg_state.is_empty() {
            PathBuf::from(expand_tilde(&cfg_state))
        } else {
            get_runtime_subdir("weixin")
        };
        let _ = std::fs::create_dir_all(&dir);
        inner.state_dir = Some(dir.clone());
        dir
    }

    async fn load_state(&self) -> bool {
        let dir = self.get_state_dir().await;
        let path = dir.join("account.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return false;
        };
        let Ok(state): Result<PersistentState, _> = serde_json::from_str(&text) else {
            return false;
        };
        let mut inner = self.inner.lock().await;
        if !state.base_url.is_empty() {
            self.config.write().await.base_url = state.base_url.clone();
        }
        let has_token = !state.token.is_empty();
        inner.state = state;
        has_token
    }

    async fn save_state(&self) {
        let dir = self.get_state_dir().await;
        let path = dir.join("account.json");
        let inner = self.inner.lock().await;
        let mut state = inner.state.clone();
        state.base_url = self.config.read().await.base_url.clone();
        if let Ok(text) = serde_json::to_string(&state) {
            let _ = std::fs::write(&path, text);
        }
    }

    // ------------------------------------------------------------------
    // HTTP helpers
    // ------------------------------------------------------------------

    fn random_wechat_uin() -> String {
        let mut buf = [0u8; 4];
        rand::thread_rng().fill_bytes(&mut buf);
        let n = u32::from_be_bytes(buf);
        B64.encode(n.to_string())
    }

    async fn make_headers(&self, auth: bool) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("X-WECHAT-UIN", Self::random_wechat_uin().parse().unwrap());
        h.insert("Content-Type", "application/json".parse().unwrap());
        h.insert("AuthorizationType", "ilink_bot_token".parse().unwrap());
        h.insert("iLink-App-Id", ILINK_APP_ID.parse().unwrap());
        h.insert(
            "iLink-App-ClientVersion",
            build_client_version(WEIXIN_CHANNEL_VERSION)
                .to_string()
                .parse()
                .unwrap(),
        );
        if auth {
            let token = self.inner.lock().await.state.token.clone();
            let authorization = if token.is_empty() {
                None
            } else {
                format!("Bearer {token}").parse().ok()
            };
            if let Some(v) = authorization {
                h.insert("Authorization", v);
            }
        }
        let route_tag = self.config.read().await.route_tag.clone();
        if let Some(tag) = route_tag {
            let tag = tag.trim().to_string();
            let route_header = if tag.is_empty() {
                None
            } else {
                tag.parse().ok()
            };
            if let Some(v) = route_header {
                h.insert("SKRouteTag", v);
            }
        }
        h
    }

    async fn api_get(
        &self,
        endpoint: &str,
        params: Option<&[(&str, &str)]>,
        auth: bool,
    ) -> Result<Value, ChannelError> {
        let base = self.config.read().await.base_url.clone();
        let url = format!("{base}/{endpoint}");
        let client = self.ensure_http(60).await?;
        let mut req = client.get(&url).headers(self.make_headers(auth).await);
        if let Some(p) = params {
            req = req.query(p);
        }
        let resp = req.send().await?.error_for_status()?;
        Ok(resp.json::<Value>().await?)
    }

    async fn api_get_with_base(
        &self,
        base_url: &str,
        endpoint: &str,
        params: Option<&[(&str, &str)]>,
        auth: bool,
    ) -> Result<Value, ChannelError> {
        let url = format!("{}/{}", base_url.trim_end_matches('/'), endpoint);
        let client = self.ensure_http(60).await?;
        let mut req = client.get(&url).headers(self.make_headers(auth).await);
        if let Some(p) = params {
            req = req.query(p);
        }
        let resp = req.send().await?.error_for_status()?;
        Ok(resp.json::<Value>().await?)
    }

    async fn api_post(
        &self,
        endpoint: &str,
        mut body: Value,
        auth: bool,
    ) -> Result<Value, ChannelError> {
        let base = self.config.read().await.base_url.clone();
        let url = format!("{base}/{endpoint}");
        let needs_base_info = body.get("base_info").is_none();
        if let Some(obj) = body.as_object_mut().filter(|_| needs_base_info) {
            obj.insert("base_info".into(), base_info());
        }
        let client = self.ensure_http(60).await?;
        let resp = client
            .post(&url)
            .headers(self.make_headers(auth).await)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        Ok(resp.json::<Value>().await?)
    }

    // ------------------------------------------------------------------
    // QR Code Login
    // ------------------------------------------------------------------

    async fn fetch_qr_code(&self) -> Result<(String, String), ChannelError> {
        let data = self
            .api_get(
                "ilink/bot/get_bot_qrcode",
                Some(&[("bot_type", "3")]),
                false,
            )
            .await?;
        let qrcode_id = data
            .get("qrcode")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if qrcode_id.is_empty() {
            return Err(ChannelError::Other(format!(
                "Failed to get QR code from WeChat API: {data}"
            )));
        }
        let img_content = data
            .get("qrcode_img_content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let scan = if img_content.is_empty() {
            qrcode_id.clone()
        } else {
            img_content
        };
        Ok((qrcode_id, scan))
    }

    async fn qr_login(&self) -> bool {
        let (mut qrcode_id, mut scan_url) = match self.fetch_qr_code().await {
            Ok(t) => t,
            Err(e) => {
                error!("WeChat QR login failed: {e}");
                return false;
            }
        };
        Self::print_qr_code(&scan_url);
        let mut current_base = self.config.read().await.base_url.clone();
        let mut refresh_count: u32 = 0;

        while self.running.load(std::sync::atomic::Ordering::SeqCst) {
            let status_data = match self
                .api_get_with_base(
                    &current_base,
                    "ilink/bot/get_qrcode_status",
                    Some(&[("qrcode", qrcode_id.as_str())]),
                    false,
                )
                .await
            {
                Ok(v) => v,
                Err(e) if Self::is_retryable_qr_poll_error(&e) => {
                    sleep(Duration::from_secs(1)).await;
                    continue;
                }
                Err(e) => {
                    error!("WeChat QR login failed: {e}");
                    return false;
                }
            };

            let status = status_data
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match status {
                "confirmed" => {
                    let token = status_data
                        .get("bot_token")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let bot_id = status_data
                        .get("ilink_bot_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let user_id = status_data
                        .get("ilink_user_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let new_base = status_data
                        .get("baseurl")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if token.is_empty() {
                        error!("Login confirmed but no bot_token in response");
                        return false;
                    }
                    self.inner.lock().await.state.token = token;
                    if !new_base.is_empty() {
                        self.config.write().await.base_url = new_base;
                    }
                    self.save_state().await;
                    info!("WeChat login successful! bot_id={bot_id} user_id={user_id}");
                    return true;
                }
                "scaned_but_redirect" => {
                    let host = status_data
                        .get("redirect_host")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if !host.is_empty() {
                        let redirected =
                            if host.starts_with("http://") || host.starts_with("https://") {
                                host
                            } else {
                                format!("https://{host}")
                            };
                        if redirected != current_base {
                            current_base = redirected;
                        }
                    }
                }
                "expired" => {
                    refresh_count += 1;
                    if refresh_count > MAX_QR_REFRESH_COUNT {
                        warn!(
                            "QR code expired too many times ({}/{}), giving up.",
                            refresh_count - 1,
                            MAX_QR_REFRESH_COUNT
                        );
                        return false;
                    }
                    match self.fetch_qr_code().await {
                        Ok((id, url)) => {
                            qrcode_id = id;
                            scan_url = url;
                            current_base = self.config.read().await.base_url.clone();
                            Self::print_qr_code(&scan_url);
                        }
                        Err(e) => {
                            error!("WeChat QR login failed: {e}");
                            return false;
                        }
                    }
                    continue;
                }
                _ => {
                    // "wait" — keep polling
                }
            }
            sleep(Duration::from_secs(1)).await;
        }

        false
    }

    fn is_retryable_qr_poll_error(err: &ChannelError) -> bool {
        match err {
            ChannelError::Http(e) => {
                if e.is_timeout() || e.is_connect() || e.is_request() {
                    return true;
                }
                if let Some(code) = e.status() {
                    return code.as_u16() >= 500;
                }
                false
            }
            ChannelError::Transient(_) => true,
            _ => false,
        }
    }

    fn print_qr_code(url: &str) {
        // Render to ASCII via the `qrcode` crate; fall back to the raw
        // URL if anything goes wrong (matches the Python fallback).
        match qrcode::QrCode::new(url.as_bytes()) {
            Ok(code) => {
                let s = code
                    .render::<char>()
                    .quiet_zone(true)
                    .module_dimensions(2, 1)
                    .build();
                println!("\n{s}\n");
            }
            Err(_) => {
                println!("\nLogin URL: {url}\n");
            }
        }
    }

    // ------------------------------------------------------------------
    // Polling
    // ------------------------------------------------------------------

    async fn pause_session(&self, duration_s: u64) {
        let until = Instant::now() + Duration::from_secs(duration_s);
        self.inner.lock().await.session_pause_until = Some(until);
    }

    async fn session_pause_remaining_s(&self) -> u64 {
        let mut inner = self.inner.lock().await;
        let Some(until) = inner.session_pause_until else {
            return 0;
        };
        let now = Instant::now();
        if until <= now {
            inner.session_pause_until = None;
            return 0;
        }
        (until - now).as_secs()
    }

    async fn poll_once(self: &Arc<Self>) -> Result<(), ChannelError> {
        let remaining = self.session_pause_remaining_s().await;
        if remaining > 0 {
            sleep(Duration::from_secs(remaining)).await;
            return Ok(());
        }

        let buf = self.inner.lock().await.state.get_updates_buf.clone();
        let body = json!({
            "get_updates_buf": buf,
            "base_info": base_info(),
        });

        let data = self.api_post("ilink/bot/getupdates", body, true).await?;
        let ret = data.get("ret").and_then(|v| v.as_i64()).unwrap_or(0);
        let errcode = data.get("errcode").and_then(|v| v.as_i64()).unwrap_or(0);
        let is_error = ret != 0 || errcode != 0;

        if is_error {
            if errcode == ERRCODE_SESSION_EXPIRED || ret == ERRCODE_SESSION_EXPIRED {
                self.pause_session(SESSION_PAUSE_DURATION_S).await;
                let remaining = self.session_pause_remaining_s().await;
                let mins = remaining.div_ceil(60).max(1);
                warn!("WeChat session expired (errcode {errcode}). Pausing {mins} min.");
                return Ok(());
            }
            let errmsg = data.get("errmsg").and_then(|v| v.as_str()).unwrap_or("");
            return Err(ChannelError::Other(format!(
                "getUpdates failed: ret={ret} errcode={errcode} errmsg={errmsg}"
            )));
        }

        if let Some(server_timeout_ms) = data
            .get("longpolling_timeout_ms")
            .and_then(|v| v.as_u64())
            .filter(|server_timeout_ms| *server_timeout_ms > 0)
        {
            self.inner.lock().await.next_poll_timeout_s = (server_timeout_ms / 1000).max(5);
        }

        let new_buf = data
            .get("get_updates_buf")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !new_buf.is_empty() {
            self.inner.lock().await.state.get_updates_buf = new_buf.to_string();
            self.save_state().await;
        }

        if let Some(msgs) = data.get("msgs").and_then(|v| v.as_array()) {
            for msg in msgs.clone() {
                if let Err(e) = self.process_message(msg).await {
                    debug!("WeChat process_message error: {e}");
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Inbound message processing
    // ------------------------------------------------------------------

    async fn process_message(self: &Arc<Self>, msg: Value) -> Result<(), ChannelError> {
        let m = msg
            .as_object()
            .ok_or_else(|| ChannelError::Other("msg not object".into()))?;
        if m.get("message_type").and_then(|v| v.as_u64()).unwrap_or(0) == MESSAGE_TYPE_BOT as u64 {
            return Ok(());
        }

        let mut msg_id = m
            .get("message_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if msg_id.is_empty() {
            msg_id = m
                .get("seq")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
        }
        if msg_id.is_empty() {
            let from = m.get("from_user_id").and_then(|v| v.as_str()).unwrap_or("");
            let ts = m
                .get("create_time_ms")
                .map(|v| v.to_string())
                .unwrap_or_default();
            msg_id = format!("{from}_{ts}");
        }

        {
            let mut inner = self.inner.lock().await;
            if inner.processed_ids.iter().any(|id| id == &msg_id) {
                return Ok(());
            }
            inner.processed_ids.push_back(msg_id.clone());
            while inner.processed_ids.len() > 1000 {
                inner.processed_ids.pop_front();
            }
        }

        let from_user_id = m
            .get("from_user_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if from_user_id.is_empty() {
            return Ok(());
        }

        let ctx_token = m
            .get("context_token")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !ctx_token.is_empty() {
            self.inner
                .lock()
                .await
                .state
                .context_tokens
                .insert(from_user_id.clone(), ctx_token.clone());
            self.save_state().await;
        }

        let item_list = m
            .get("item_list")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut content_parts: Vec<String> = Vec::new();
        let mut media_paths: Vec<String> = Vec::new();
        let mut has_top_level_downloadable_media = false;

        for item in &item_list {
            let item_type = item.get("type").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            match item_type {
                t if t == ITEM_TEXT => {
                    let text = item
                        .get("text_item")
                        .and_then(|v| v.get("text"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if text.is_empty() {
                        continue;
                    }
                    if let Some(refmsg) = item.get("ref_msg") {
                        let ref_item = refmsg.get("message_item");
                        let ref_type = ref_item
                            .and_then(|v| v.get("type"))
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32;
                        if matches!(ref_type, t if t == ITEM_IMAGE || t == ITEM_VOICE || t == ITEM_FILE || t == ITEM_VIDEO)
                        {
                            content_parts.push(text);
                        } else {
                            let mut parts: Vec<String> = Vec::new();
                            if let Some(title) = refmsg
                                .get("title")
                                .and_then(|v| v.as_str())
                                .filter(|title| !title.is_empty())
                            {
                                parts.push(title.to_string());
                            }
                            if let Some(ri) = ref_item {
                                let ref_text = ri
                                    .get("text_item")
                                    .and_then(|v| v.get("text"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                if !ref_text.is_empty() {
                                    parts.push(ref_text.to_string());
                                }
                            }
                            if parts.is_empty() {
                                content_parts.push(text);
                            } else {
                                content_parts
                                    .push(format!("[引用: {}]\n{text}", parts.join(" | ")));
                            }
                        }
                    } else {
                        content_parts.push(text);
                    }
                }
                t if t == ITEM_IMAGE => {
                    let image_item = item.get("image_item").cloned().unwrap_or(Value::Null);
                    if has_downloadable_media_locator(image_item.get("media")) {
                        has_top_level_downloadable_media = true;
                    }
                    let path = self.download_media_item(&image_item, "image", None).await;
                    match path {
                        Some(p) => {
                            content_parts.push(format!("[image]\n[Image: source: {p}]"));
                            media_paths.push(p);
                        }
                        None => content_parts.push("[image]".into()),
                    }
                }
                t if t == ITEM_VOICE => {
                    let voice_item = item.get("voice_item").cloned().unwrap_or(Value::Null);
                    let voice_text = voice_item
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if !voice_text.is_empty() {
                        content_parts.push(format!("[voice] {voice_text}"));
                    } else {
                        if has_downloadable_media_locator(voice_item.get("media")) {
                            has_top_level_downloadable_media = true;
                        }
                        let path = self.download_media_item(&voice_item, "voice", None).await;
                        match path {
                            Some(p) => {
                                let transcription = self.transcribe_audio_helper(&p).await;
                                if !transcription.is_empty() {
                                    content_parts.push(format!("[voice] {transcription}"));
                                } else {
                                    content_parts.push(format!("[voice]\n[Audio: source: {p}]"));
                                }
                                media_paths.push(p);
                            }
                            None => content_parts.push("[voice]".into()),
                        }
                    }
                }
                t if t == ITEM_FILE => {
                    let file_item = item.get("file_item").cloned().unwrap_or(Value::Null);
                    if has_downloadable_media_locator(file_item.get("media")) {
                        has_top_level_downloadable_media = true;
                    }
                    let file_name = file_item
                        .get("file_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let path = self
                        .download_media_item(&file_item, "file", Some(&file_name))
                        .await;
                    match path {
                        Some(p) => {
                            content_parts.push(format!("[file: {file_name}]\n[File: source: {p}]"));
                            media_paths.push(p);
                        }
                        None => content_parts.push(format!("[file: {file_name}]")),
                    }
                }
                t if t == ITEM_VIDEO => {
                    let video_item = item.get("video_item").cloned().unwrap_or(Value::Null);
                    if has_downloadable_media_locator(video_item.get("media")) {
                        has_top_level_downloadable_media = true;
                    }
                    let path = self.download_media_item(&video_item, "video", None).await;
                    match path {
                        Some(p) => {
                            content_parts.push(format!("[video]\n[Video: source: {p}]"));
                            media_paths.push(p);
                        }
                        None => content_parts.push("[video]".into()),
                    }
                }
                _ => {}
            }
        }

        // Fallback: try referenced media when nothing top-level was downloaded.
        if media_paths.is_empty() && !has_top_level_downloadable_media {
            for item in &item_list {
                if item.get("type").and_then(|v| v.as_u64()).unwrap_or(0) as u32 != ITEM_TEXT {
                    continue;
                }
                let candidate = item
                    .get("ref_msg")
                    .and_then(|v| v.get("message_item"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let ref_type = candidate.get("type").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                match ref_type {
                    t if t == ITEM_IMAGE => {
                        let image_item =
                            candidate.get("image_item").cloned().unwrap_or(Value::Null);
                        if let Some(p) = self.download_media_item(&image_item, "image", None).await
                        {
                            content_parts.push(format!("[image]\n[Image: source: {p}]"));
                            media_paths.push(p);
                            break;
                        }
                    }
                    t if t == ITEM_VOICE => {
                        let voice_item =
                            candidate.get("voice_item").cloned().unwrap_or(Value::Null);
                        if let Some(p) = self.download_media_item(&voice_item, "voice", None).await
                        {
                            let transcription = self.transcribe_audio_helper(&p).await;
                            if !transcription.is_empty() {
                                content_parts.push(format!("[voice] {transcription}"));
                            } else {
                                content_parts.push(format!("[voice]\n[Audio: source: {p}]"));
                            }
                            media_paths.push(p);
                            break;
                        }
                    }
                    t if t == ITEM_FILE => {
                        let file_item = candidate.get("file_item").cloned().unwrap_or(Value::Null);
                        let file_name = file_item
                            .get("file_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        if let Some(p) = self
                            .download_media_item(&file_item, "file", Some(&file_name))
                            .await
                        {
                            content_parts.push(format!("[file: {file_name}]\n[File: source: {p}]"));
                            media_paths.push(p);
                            break;
                        }
                    }
                    t if t == ITEM_VIDEO => {
                        let video_item =
                            candidate.get("video_item").cloned().unwrap_or(Value::Null);
                        if let Some(p) = self.download_media_item(&video_item, "video", None).await
                        {
                            content_parts.push(format!("[video]\n[Video: source: {p}]"));
                            media_paths.push(p);
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }

        let content = content_parts.join("\n");
        if content.is_empty() {
            return Ok(());
        }

        info!(
            "WeChat inbound: from={} items={} bodyLen={}",
            from_user_id,
            item_list
                .iter()
                .map(|i| i
                    .get("type")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    .to_string())
                .collect::<Vec<_>>()
                .join(","),
            content.len()
        );

        // Start typing indicator (best effort).
        let this = Arc::clone(self);
        let chat_id_for_typing = from_user_id.clone();
        let ctx_token_for_typing = ctx_token.clone();
        tokio::spawn(async move {
            this.start_typing(&chat_id_for_typing, &ctx_token_for_typing)
                .await;
        });

        let mut metadata = Map::new();
        metadata.insert("message_id".into(), Value::String(msg_id.clone()));

        let allow_list = self.config.read().await.allow_from.clone();
        handle_inbound(
            &self.bus,
            WeixinChannel::name(),
            &allow_list,
            self.supports_streaming(),
            from_user_id.clone(),
            from_user_id,
            content,
            media_paths,
            Some(metadata),
            None,
        )
        .await;

        Ok(())
    }

    async fn transcribe_audio_helper(&self, file_path: &str) -> String {
        let settings = self.transcription.read().await.clone();
        transcribe_audio(WeixinChannel::name(), &settings, Path::new(file_path)).await
    }

    // ------------------------------------------------------------------
    // Media download
    // ------------------------------------------------------------------

    async fn download_media_item(
        &self,
        typed_item: &Value,
        media_type: &str,
        filename: Option<&str>,
    ) -> Option<String> {
        let result = self
            .download_media_item_inner(typed_item, media_type, filename)
            .await;
        if let Err(e) = &result {
            error!("Error downloading WeChat media: {e}");
        }
        result.ok().flatten()
    }

    async fn download_media_item_inner(
        &self,
        typed_item: &Value,
        media_type: &str,
        filename: Option<&str>,
    ) -> Result<Option<String>, ChannelError> {
        let media = typed_item.get("media").cloned().unwrap_or(Value::Null);
        let encrypt_query_param = media
            .get("encrypt_query_param")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let full_url = media
            .get("full_url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if encrypt_query_param.is_empty() && full_url.is_empty() {
            return Ok(None);
        }

        let raw_aeskey_hex = typed_item
            .get("aeskey")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let media_aes_key_b64 = media.get("aes_key").and_then(|v| v.as_str()).unwrap_or("");

        let aes_key_b64: String = if !raw_aeskey_hex.is_empty() {
            match hex::decode(raw_aeskey_hex) {
                Ok(bytes) => B64.encode(bytes),
                Err(_) => String::new(),
            }
        } else {
            media_aes_key_b64.to_string()
        };

        if media_type != "image" && aes_key_b64.is_empty() {
            return Ok(None);
        }

        let cdn_base = self.config.read().await.cdn_base_url.clone();
        let fallback_url = if !encrypt_query_param.is_empty() {
            format!(
                "{cdn_base}/download?encrypted_query_param={}",
                urlencoding::encode(&encrypt_query_param)
            )
        } else {
            String::new()
        };

        let mut candidates: Vec<(&'static str, String)> = Vec::new();
        if !full_url.is_empty() {
            candidates.push(("full_url", full_url.clone()));
        }
        if !fallback_url.is_empty() && (full_url.is_empty() || fallback_url != full_url) {
            candidates.push(("encrypt_query_param", fallback_url));
        }

        let client = self.ensure_http(120).await?;
        let mut data = Vec::<u8>::new();
        for (i, (source, url)) in candidates.iter().enumerate() {
            match client
                .get(url)
                .send()
                .await
                .and_then(|r| r.error_for_status())
            {
                Ok(resp) => match resp.bytes().await {
                    Ok(b) => {
                        data = b.to_vec();
                        break;
                    }
                    Err(e) => {
                        return Err(ChannelError::Http(e));
                    }
                },
                Err(e) => {
                    let has_more = i + 1 < candidates.len();
                    let retryable = Self::is_retryable_media_download_reqwest(&e);
                    if *source == "full_url" && has_more && retryable {
                        warn!(
                            "WeChat media download failed via full_url, falling back to encrypt_query_param: type={media_type} err={e}"
                        );
                        continue;
                    }
                    return Err(ChannelError::Http(e));
                }
            }
        }

        if !aes_key_b64.is_empty() && !data.is_empty() {
            data = decrypt_aes_ecb(&data, &aes_key_b64);
        }
        if data.is_empty() {
            return Ok(None);
        }

        let media_dir = get_media_dir(Some("weixin"));
        let ext = ext_for_type(media_type);
        let filename = match filename {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => {
                let ts = unix_secs();
                let seed = if !encrypt_query_param.is_empty() {
                    encrypt_query_param.clone()
                } else {
                    full_url.clone()
                };
                let h = stable_hash(&seed) % 100000;
                format!("{media_type}_{ts}_{h}{ext}")
            }
        };
        let safe = std::path::Path::new(&filename)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("file")
            .to_string();
        let file_path = media_dir.join(safe);
        fs::write(&file_path, &data).await?;
        Ok(Some(file_path.to_string_lossy().into_owned()))
    }

    fn is_retryable_media_download_reqwest(err: &reqwest::Error) -> bool {
        if err.is_timeout() || err.is_connect() || err.is_request() {
            return true;
        }
        if let Some(c) = err.status() {
            return c.as_u16() >= 500;
        }
        false
    }

    // ------------------------------------------------------------------
    // Outbound
    // ------------------------------------------------------------------

    async fn get_typing_ticket(&self, user_id: &str, context_token: &str) -> String {
        let now = unix_secs_f();
        {
            let inner = self.inner.lock().await;
            if let Some(entry) = inner
                .state
                .typing_tickets
                .get(user_id)
                .filter(|entry| now < entry.next_fetch_at)
            {
                return entry.ticket.clone();
            }
        }

        let body = json!({
            "ilink_user_id": user_id,
            "context_token": if context_token.is_empty() { Value::Null } else { Value::String(context_token.into()) },
            "base_info": base_info(),
        });

        let data = match self.api_post("ilink/bot/getconfig", body, true).await {
            Ok(d) => d,
            Err(_) => return String::new(),
        };
        let ret = data.get("ret").and_then(|v| v.as_i64()).unwrap_or(-1);
        let mut inner = self.inner.lock().await;
        if ret == 0 {
            let ticket = data
                .get("typing_ticket")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // Random jitter [0, TYPING_TICKET_TTL_S).
            let jitter = rand::random::<f64>() * TYPING_TICKET_TTL_S as f64;
            inner.state.typing_tickets.insert(
                user_id.into(),
                TicketEntry {
                    ticket: ticket.clone(),
                    ever_succeeded: true,
                    next_fetch_at: now + jitter,
                    retry_delay_s: CONFIG_CACHE_INITIAL_RETRY_S as f64,
                },
            );
            return ticket;
        }

        let prev = inner
            .state
            .typing_tickets
            .get(user_id)
            .map(|e| e.retry_delay_s)
            .unwrap_or(CONFIG_CACHE_INITIAL_RETRY_S as f64);
        let next_delay = (prev * 2.0).min(CONFIG_CACHE_MAX_RETRY_S as f64);
        let entry = inner
            .state
            .typing_tickets
            .entry(user_id.into())
            .or_default();
        entry.next_fetch_at = now + next_delay;
        entry.retry_delay_s = next_delay;
        entry.ticket.clone()
    }

    async fn send_typing(
        &self,
        user_id: &str,
        ticket: &str,
        status: u32,
    ) -> Result<(), ChannelError> {
        if ticket.is_empty() {
            return Ok(());
        }
        let body = json!({
            "ilink_user_id": user_id,
            "typing_ticket": ticket,
            "status": status,
            "base_info": base_info(),
        });
        let _ = self.api_post("ilink/bot/sendtyping", body, true).await?;
        Ok(())
    }

    async fn start_typing(self: Arc<Self>, chat_id: &str, ctx_token: &str) {
        if chat_id.is_empty() {
            return;
        }
        if self.inner.lock().await.state.token.is_empty() {
            return;
        }
        self.stop_typing(chat_id, false).await;
        let ticket = self.get_typing_ticket(chat_id, ctx_token).await;
        if ticket.is_empty() {
            return;
        }
        if let Err(e) = self
            .send_typing(chat_id, &ticket, TYPING_STATUS_TYPING)
            .await
        {
            debug!("WeChat typing indicator start failed for {chat_id}: {e}");
            return;
        }

        let stop = Arc::new(Notify::new());
        let stop_clone = stop.clone();
        let this = Arc::clone(&self);
        let chat = chat_id.to_string();
        let ticket_owned = ticket.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop_clone.notified() => break,
                    _ = sleep(Duration::from_secs(TYPING_KEEPALIVE_INTERVAL_S)) => {
                        let _ = this.send_typing(&chat, &ticket_owned, TYPING_STATUS_TYPING).await;
                    }
                }
            }
        });
        self.typing
            .lock()
            .await
            .insert(chat_id.to_string(), TypingTask { stop, handle });
    }

    async fn stop_typing(&self, chat_id: &str, clear_remote: bool) {
        let task = self.typing.lock().await.remove(chat_id);
        if let Some(t) = task {
            t.stop.notify_one();
            t.handle.abort();
        }
        if !clear_remote {
            return;
        }
        let ticket = {
            let inner = self.inner.lock().await;
            inner
                .state
                .typing_tickets
                .get(chat_id)
                .map(|e| e.ticket.clone())
                .unwrap_or_default()
        };
        if ticket.is_empty() {
            return;
        }
        if let Err(e) = self
            .send_typing(chat_id, &ticket, TYPING_STATUS_CANCEL)
            .await
        {
            debug!("WeChat typing clear failed for {chat_id}: {e}");
        }
    }

    async fn send_text(
        &self,
        to_user_id: &str,
        text: &str,
        context_token: &str,
    ) -> Result<(), ChannelError> {
        let client_id = format!("nanobot-{}", &Uuid::new_v4().simple().to_string()[..12]);
        let mut item_list: Vec<Value> = Vec::new();
        if !text.is_empty() {
            item_list.push(json!({
                "type": ITEM_TEXT,
                "text_item": { "text": text },
            }));
        }
        let mut weixin_msg = json!({
            "from_user_id": "",
            "to_user_id": to_user_id,
            "client_id": client_id,
            "message_type": MESSAGE_TYPE_BOT,
            "message_state": MESSAGE_STATE_FINISH,
        });
        if !item_list.is_empty() {
            weixin_msg["item_list"] = Value::Array(item_list);
        }
        if !context_token.is_empty() {
            weixin_msg["context_token"] = Value::String(context_token.into());
        }
        let body = json!({ "msg": weixin_msg, "base_info": base_info() });
        let data = self.api_post("ilink/bot/sendmessage", body, true).await?;
        let errcode = data.get("errcode").and_then(|v| v.as_i64()).unwrap_or(0);
        if errcode != 0 {
            warn!(
                "WeChat send error (code {errcode}): {}",
                data.get("errmsg").and_then(|v| v.as_str()).unwrap_or("")
            );
        }
        Ok(())
    }

    async fn send_media_file(
        &self,
        to_user_id: &str,
        media_path: &str,
        context_token: &str,
    ) -> Result<(), ChannelError> {
        let p = std::path::Path::new(media_path);
        if !p.is_file() {
            return Err(ChannelError::Other(format!(
                "Media file not found: {media_path}"
            )));
        }
        let raw_data = fs::read(p).await?;
        let raw_size = raw_data.len();
        let raw_md5 = {
            use md5::{Digest, Md5};
            let mut h = Md5::new();
            h.update(&raw_data);
            hex::encode(h.finalize())
        };

        let ext = p
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| format!(".{}", e.to_lowercase()))
            .unwrap_or_default();

        let (upload_type, item_type, item_key) = if image_exts().contains(&ext.as_str()) {
            (UPLOAD_MEDIA_IMAGE, ITEM_IMAGE, "image_item")
        } else if video_exts().contains(&ext.as_str()) {
            (UPLOAD_MEDIA_VIDEO, ITEM_VIDEO, "video_item")
        } else if voice_exts().contains(&ext.as_str()) {
            (UPLOAD_MEDIA_VOICE, ITEM_VOICE, "voice_item")
        } else {
            (UPLOAD_MEDIA_FILE, ITEM_FILE, "file_item")
        };

        let mut aes_key_raw = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut aes_key_raw);
        let aes_key_hex = hex::encode(aes_key_raw);

        // PKCS7 padding -> ceil((size+1)/16)*16
        let padded_size = (raw_size + 1).div_ceil(16) * 16;

        let mut file_key_raw = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut file_key_raw);
        let file_key = hex::encode(file_key_raw);

        let upload_body = json!({
            "filekey": file_key,
            "media_type": upload_type,
            "to_user_id": to_user_id,
            "rawsize": raw_size,
            "rawfilemd5": raw_md5,
            "filesize": padded_size,
            "no_need_thumb": true,
            "aeskey": aes_key_hex,
        });

        let upload_resp = self
            .api_post("ilink/bot/getuploadurl", upload_body, true)
            .await?;
        let upload_full_url = upload_resp
            .get("upload_full_url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let upload_param = upload_resp
            .get("upload_param")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if upload_full_url.is_empty() && upload_param.is_empty() {
            return Err(ChannelError::Other(format!(
                "getuploadurl returned no upload URL: {upload_resp}"
            )));
        }

        let aes_key_b64 = B64.encode(aes_key_raw);
        let encrypted = encrypt_aes_ecb(&raw_data, &aes_key_b64);

        let cdn_base = self.config.read().await.cdn_base_url.clone();
        let cdn_upload_url = if !upload_full_url.is_empty() {
            upload_full_url
        } else {
            format!(
                "{cdn_base}/upload?encrypted_query_param={}&filekey={}",
                urlencoding::encode(&upload_param),
                urlencoding::encode(&file_key)
            )
        };

        let client = self.ensure_http(120).await?;
        let cdn_resp = client
            .post(&cdn_upload_url)
            .header("Content-Type", "application/octet-stream")
            .body(encrypted)
            .send()
            .await?
            .error_for_status()?;
        let download_param = cdn_resp
            .headers()
            .get("x-encrypted-param")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if download_param.is_empty() {
            return Err(ChannelError::Other(format!(
                "CDN upload response missing x-encrypted-param header; status={}",
                cdn_resp.status()
            )));
        }

        let cdn_aes_key_b64 = B64.encode(aes_key_hex.as_bytes());
        let mut media_item = json!({
            "media": {
                "encrypt_query_param": download_param,
                "aes_key": cdn_aes_key_b64,
                "encrypt_type": 1,
            },
        });
        match item_type {
            t if t == ITEM_IMAGE => {
                media_item["mid_size"] = json!(padded_size);
            }
            t if t == ITEM_VIDEO => {
                media_item["video_size"] = json!(padded_size);
            }
            t if t == ITEM_FILE => {
                media_item["file_name"] =
                    json!(p.file_name().and_then(|s| s.to_str()).unwrap_or(""));
                media_item["len"] = json!(raw_size.to_string());
            }
            _ => {}
        }

        let client_id = format!("nanobot-{}", &Uuid::new_v4().simple().to_string()[..12]);
        let item_list = vec![json!({ "type": item_type, item_key: media_item })];
        let mut weixin_msg = json!({
            "from_user_id": "",
            "to_user_id": to_user_id,
            "client_id": client_id,
            "message_type": MESSAGE_TYPE_BOT,
            "message_state": MESSAGE_STATE_FINISH,
            "item_list": item_list,
        });
        if !context_token.is_empty() {
            weixin_msg["context_token"] = Value::String(context_token.into());
        }
        let body = json!({ "msg": weixin_msg, "base_info": base_info() });
        let data = self.api_post("ilink/bot/sendmessage", body, true).await?;
        let errcode = data.get("errcode").and_then(|v| v.as_i64()).unwrap_or(0);
        if errcode != 0 {
            return Err(ChannelError::Other(format!(
                "WeChat send media error (code {errcode}): {}",
                data.get("errmsg").and_then(|v| v.as_str()).unwrap_or("")
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl Channel for WeixinChannel {
    fn name() -> &'static str {
        "weixin"
    }
    fn display_name() -> &'static str {
        "WeChat"
    }
    fn bus(&self) -> &MessageBus {
        &self.bus
    }
    fn is_running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn default_config() -> serde_json::Map<String, Value> {
        serde_json::to_value(WeixinConfig::default())
            .ok()
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    }

    async fn login(self: Arc<Self>, force: bool) -> ChannelResult<bool> {
        if force {
            self.inner.lock().await.state.token.clear();
            self.inner.lock().await.state.get_updates_buf.clear();
            let path = self.get_state_dir().await.join("account.json");
            let _ = std::fs::remove_file(&path);
        }
        if !self.inner.lock().await.state.token.is_empty() || self.load_state().await {
            return Ok(true);
        }
        // Init HTTP for the login flow
        let _ = self.ensure_http(60).await?;
        self.running
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let ok = self.qr_login().await;
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(ok)
    }

    async fn start(self: Arc<Self>) -> ChannelResult<()> {
        self.running
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let poll_to = self.config.read().await.poll_timeout;
        self.inner.lock().await.next_poll_timeout_s = poll_to;
        // Build HTTP with a generous total timeout (poll + 10s buffer).
        {
            let mut g = self.http.lock().await;
            *g = Some(self.http_client(poll_to + 10)?);
        }

        let cfg_token = self.config.read().await.token.clone();
        if !cfg_token.is_empty() {
            self.inner.lock().await.state.token = cfg_token;
        } else if !self.load_state().await && !self.qr_login().await {
            error!("WeChat login failed. Run 'nanobot channels login weixin' to authenticate.");
            self.running
                .store(false, std::sync::atomic::Ordering::SeqCst);
            return Ok(());
        }

        info!("WeChat channel starting with long-poll...");

        let mut consecutive_failures: u32 = 0;
        while self.running.load(std::sync::atomic::Ordering::SeqCst) {
            match self.poll_once().await {
                Ok(()) => {
                    consecutive_failures = 0;
                }
                Err(ChannelError::Http(e)) if e.is_timeout() => {
                    // Long-poll timeouts are expected.
                    continue;
                }
                Err(e) => {
                    if !self.running.load(std::sync::atomic::Ordering::SeqCst) {
                        break;
                    }
                    debug!("WeChat poll error: {e}");
                    consecutive_failures += 1;
                    if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                        consecutive_failures = 0;
                        sleep(Duration::from_secs(BACKOFF_DELAY_S)).await;
                    } else {
                        sleep(Duration::from_secs(RETRY_DELAY_S)).await;
                    }
                }
            }
        }
        Ok(())
    }

    async fn stop(self: Arc<Self>) -> ChannelResult<()> {
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
        let chats: Vec<String> = self.typing.lock().await.keys().cloned().collect();
        for chat in chats {
            self.stop_typing(&chat, false).await;
        }
        *self.http.lock().await = None;
        self.save_state().await;
        Ok(())
    }

    async fn send(self: Arc<Self>, msg: OutboundMessage) -> ChannelResult<()> {
        if self.inner.lock().await.state.token.is_empty() {
            warn!("WeChat client not initialized or not authenticated");
            return Ok(());
        }
        if self.session_pause_remaining_s().await > 0 {
            return Ok(());
        }
        let is_progress = msg
            .metadata
            .get("_progress")
            .map(|v| v.as_bool().unwrap_or(false))
            .unwrap_or(false);
        if !is_progress {
            self.stop_typing(&msg.chat_id, true).await;
        }

        let content = msg.content.trim().to_string();
        let ctx_token = self
            .inner
            .lock()
            .await
            .state
            .context_tokens
            .get(&msg.chat_id)
            .cloned()
            .unwrap_or_default();
        if ctx_token.is_empty() {
            warn!(
                "WeChat: no context_token for chat_id={}, cannot send",
                msg.chat_id
            );
            return Ok(());
        }

        let typing_ticket = self.get_typing_ticket(&msg.chat_id, &ctx_token).await;
        if !typing_ticket.is_empty() {
            let _ = self
                .send_typing(&msg.chat_id, &typing_ticket, TYPING_STATUS_TYPING)
                .await;
        }

        // Background typing keepalive while we send.
        let stop = Arc::new(Notify::new());
        let stop_clone = stop.clone();
        let keepalive_handle = if !typing_ticket.is_empty() {
            let this = Arc::clone(&self);
            let chat = msg.chat_id.clone();
            let ticket = typing_ticket.clone();
            Some(tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = stop_clone.notified() => break,
                        _ = sleep(Duration::from_secs(TYPING_KEEPALIVE_INTERVAL_S)) => {
                            let _ = this.send_typing(&chat, &ticket, TYPING_STATUS_TYPING).await;
                        }
                    }
                }
            }))
        } else {
            None
        };

        let mut send_err: Option<ChannelError> = None;
        // Send media first.
        for media in msg.media.iter() {
            match self.send_media_file(&msg.chat_id, media, &ctx_token).await {
                Ok(()) => {}
                Err(e) if e.is_retryable() => {
                    error!("Network error sending WeChat media {media}: {e}");
                    send_err = Some(e);
                    break;
                }
                Err(e) => {
                    let filename = std::path::Path::new(media)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(media);
                    error!("Failed to send WeChat media {media}: {e}");
                    let _ = self
                        .send_text(
                            &msg.chat_id,
                            &format!("[Failed to send: {filename}]"),
                            &ctx_token,
                        )
                        .await;
                }
            }
        }

        if send_err.is_none() && !content.is_empty() {
            for chunk in split_message(&content, WEIXIN_MAX_MESSAGE_LEN) {
                if let Err(e) = self.send_text(&msg.chat_id, &chunk, &ctx_token).await {
                    send_err = Some(e);
                    break;
                }
            }
        }

        if let Some(h) = keepalive_handle {
            stop.notify_one();
            h.abort();
        }
        if !typing_ticket.is_empty() && !is_progress {
            let _ = self
                .send_typing(&msg.chat_id, &typing_ticket, TYPING_STATUS_CANCEL)
                .await;
        }

        if let Some(e) = send_err {
            error!("Error sending WeChat message: {e}");
            return Err(e);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// AES-128-ECB encryption / decryption
// ---------------------------------------------------------------------------

fn parse_aes_key(b64: &str) -> Option<[u8; 16]> {
    let decoded = B64.decode(b64).ok()?;
    if decoded.len() == 16 {
        let mut k = [0u8; 16];
        k.copy_from_slice(&decoded);
        return Some(k);
    }
    if decoded.len() == 32 && decoded.iter().all(|c| c.is_ascii_hexdigit()) {
        let s = std::str::from_utf8(&decoded).ok()?;
        let bytes = hex::decode(s).ok()?;
        if bytes.len() == 16 {
            let mut k = [0u8; 16];
            k.copy_from_slice(&bytes);
            return Some(k);
        }
    }
    None
}

fn encrypt_aes_ecb(data: &[u8], aes_key_b64: &str) -> Vec<u8> {
    let Some(key) = parse_aes_key(aes_key_b64) else {
        warn!("Failed to parse AES key for encryption, sending raw");
        return data.to_vec();
    };
    let pad_len = 16 - data.len() % 16;
    let mut padded = Vec::with_capacity(data.len() + pad_len);
    padded.extend_from_slice(data);
    padded.extend(std::iter::repeat_n(pad_len as u8, pad_len));

    let cipher = Aes128::new(GenericArray::from_slice(&key));
    for block in padded.chunks_mut(16) {
        let arr = GenericArray::from_mut_slice(block);
        cipher.encrypt_block(arr);
    }
    padded
}

fn decrypt_aes_ecb(data: &[u8], aes_key_b64: &str) -> Vec<u8> {
    let Some(key) = parse_aes_key(aes_key_b64) else {
        warn!("Failed to parse AES key, returning raw data");
        return data.to_vec();
    };
    if !data.chunks_exact(16).remainder().is_empty() {
        return data.to_vec();
    }
    let cipher = Aes128::new(GenericArray::from_slice(&key));
    let mut out = data.to_vec();
    for block in out.chunks_mut(16) {
        let arr = GenericArray::from_mut_slice(block);
        cipher.decrypt_block(arr);
    }
    pkcs7_unpad_safe(out)
}

fn pkcs7_unpad_safe(data: Vec<u8>) -> Vec<u8> {
    if data.is_empty() || !data.chunks_exact(16).remainder().is_empty() {
        return data;
    }
    let pad_len = *data.last().unwrap() as usize;
    if pad_len == 0 || pad_len > 16 {
        return data;
    }
    let len = data.len();
    if data[len - pad_len..].iter().all(|&b| b as usize == pad_len) {
        let mut d = data;
        d.truncate(len - pad_len);
        return d;
    }
    data
}

fn ext_for_type(t: &str) -> &'static str {
    match t {
        "image" => ".jpg",
        "voice" => ".silk",
        "video" => ".mp4",
        _ => "",
    }
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn unix_secs_f() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn stable_hash(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
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
    let ch = WeixinChannel::from_value(section, bus, transcription)?;
    let arc: Arc<dyn Channel> = Arc::new(ch);
    Ok(ChannelEntry {
        name: "weixin".into(),
        display_name: "WeChat".into(),
        channel: arc,
    })
}
