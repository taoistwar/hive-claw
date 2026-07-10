//! QQ Bot v2 WebSocket gateway client.
//!
//! Implements the protocol documented at
//! <https://bot.q.qq.com/wiki/develop/api-v2/dev-prepare/interface-framework/event-emit.html>
//!
//! Op codes (subset we use):
//!
//! | Op | Direction | Purpose                  |
//! |----|-----------|--------------------------|
//! |  0 | Receive   | Dispatch event           |
//! |  1 | Send      | Heartbeat                |
//! |  2 | Send      | Identify                 |
//! |  6 | Send      | Resume                   |
//! |  7 | Receive   | Reconnect                |
//! |  9 | Receive   | Invalid Session          |
//! | 10 | Receive   | Hello (heartbeat_interval)|
//! | 11 | Receive   | Heartbeat ACK            |
//! | 13 | Receive   | HTTP callback ACK (n/a)  |
//!
//! Only the C2C / public message intents are wired up by default
//! (`PUBLIC_MESSAGES = 1 << 25`, `DIRECT_MESSAGE = 1 << 12`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use log::{debug, error, info, warn};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};
use tokio::time::{interval, sleep};
use tokio_tungstenite::tungstenite::Message;

/// Intents bitmask. Public messages + direct messages are enough for
/// the (C2C + Group @bot + Direct) subset that the Python channel uses.
pub const INTENT_PUBLIC_MESSAGES: u32 = 1 << 25;
pub const INTENT_DIRECT_MESSAGE: u32 = 1 << 12;
pub const DEFAULT_INTENTS: u32 = INTENT_PUBLIC_MESSAGES | INTENT_DIRECT_MESSAGE;

/// Public event types we forward to the channel.
#[derive(Debug, Clone)]
pub enum GatewayEvent {
    /// `C2C_MESSAGE_CREATE` — direct user-to-bot message in QQ.
    C2cMessage(Value),
    /// `GROUP_AT_MESSAGE_CREATE` — `@bot` mention in a QQ group.
    GroupAtMessage(Value),
    /// `DIRECT_MESSAGE_CREATE` — guild direct message (legacy).
    DirectMessage(Value),
    /// Connection became ready (`READY` dispatch).
    Ready {
        session_id: String,
        bot_name: String,
    },
}

/// Authentication config required to identify against the gateway.
#[derive(Debug, Clone)]
pub struct GatewayAuth {
    pub app_id: String,
    /// `QQBot <access_token>` — refreshed by the channel.
    pub access_token: String,
    pub intents: u32,
}

/// Run a single gateway connection until it exits or the `running`
/// flag flips to false. Returns `Ok(false)` to mean "session is dead,
/// drop it and Identify again", `Ok(true)` to mean "Resume succeeded
/// or graceful close", `Err` for transport errors.
///
/// `session_state` carries `(session_id, last_seq)` between connections
/// so the caller can ask us to Resume.
pub async fn run_session(
    ws_url: &str,
    auth: &GatewayAuth,
    session_state: Arc<Mutex<Option<SessionState>>>,
    running: Arc<AtomicBool>,
    tx: mpsc::Sender<GatewayEvent>,
) -> Result<bool, GatewayError> {
    info!("QQ gateway: connecting to {ws_url}");
    let (ws, _resp) = tokio_tungstenite::connect_async(ws_url)
        .await
        .map_err(|e| GatewayError::Transport(format!("connect: {e}")))?;
    let (mut write, mut read) = ws.split();

    // Read Hello to learn heartbeat_interval.
    let hello = next_payload(&mut read).await?;
    let heartbeat_ms = hello
        .get("d")
        .and_then(|d| d.get("heartbeat_interval"))
        .and_then(|v| v.as_u64())
        .unwrap_or(40_000);
    debug!("QQ gateway: hello received, heartbeat={heartbeat_ms}ms");

    // Send Identify or Resume depending on whether we have a session.
    let resume_attempt = {
        let s = session_state.lock().await;
        s.clone()
    };
    let resuming = match &resume_attempt {
        Some(state) => {
            let payload = json!({
                "op": 6,
                "d": {
                    "token": format!("QQBot {}", auth.access_token),
                    "session_id": state.session_id,
                    "seq": state.last_seq,
                }
            });
            send_json(&mut write, &payload).await?;
            true
        }
        None => {
            let payload = json!({
                "op": 2,
                "d": {
                    "token": format!("QQBot {}", auth.access_token),
                    "intents": auth.intents,
                    "shard": [0, 1],
                    "properties": {
                        "$os": std::env::consts::OS,
                        "$browser": "nanobot-rust",
                        "$device": "nanobot-rust",
                    }
                }
            });
            send_json(&mut write, &payload).await?;
            false
        }
    };

    // Heartbeat task — sends `{"op":1,"d":<last_seq>}` every interval.
    let writer = Arc::new(Mutex::new(write));
    let heartbeat_state = session_state.clone();
    let heartbeat_writer = writer.clone();
    let heartbeat_running = running.clone();
    let heartbeat_handle = tokio::spawn(async move {
        let mut tick = interval(Duration::from_millis(heartbeat_ms));
        // Skip the immediate first tick (we just connected).
        tick.tick().await;
        while heartbeat_running.load(Ordering::SeqCst) {
            tick.tick().await;
            let seq = heartbeat_state
                .lock()
                .await
                .as_ref()
                .map(|s| Value::from(s.last_seq))
                .unwrap_or(Value::Null);
            let payload = json!({"op": 1, "d": seq});
            let mut w = heartbeat_writer.lock().await;
            if w.send(Message::Text(payload.to_string())).await.is_err() {
                break;
            }
        }
    });

    let mut graceful = false;
    let mut session_invalid = false;

    while running.load(Ordering::SeqCst) {
        let payload = match next_payload(&mut read).await {
            Ok(v) => v,
            Err(GatewayError::Closed) => {
                graceful = true;
                break;
            }
            Err(e) => {
                heartbeat_handle.abort();
                return Err(e);
            }
        };
        let op = payload.get("op").and_then(|v| v.as_u64()).unwrap_or(0);
        match op {
            0 => {
                // Dispatch — update seq + route by `t`.
                if let Some(seq) = payload.get("s").and_then(|v| v.as_u64()) {
                    let mut s = session_state.lock().await;
                    let entry = s.get_or_insert_with(SessionState::default);
                    entry.last_seq = seq;
                }
                let t = payload
                    .get("t")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let d = payload.get("d").cloned().unwrap_or(Value::Null);
                if let Err(e) = dispatch_event(&t, d, &session_state, &tx).await {
                    debug!("QQ gateway: dispatch failed: {e}");
                }
            }
            7 => {
                // Reconnect requested — close gracefully and let caller
                // re-enter with the same session_state (Resume path).
                info!("QQ gateway: reconnect requested by server");
                graceful = true;
                break;
            }
            9 => {
                // Invalid session — wipe state, sleep, then re-identify.
                let resumable = payload.get("d").and_then(|v| v.as_bool()).unwrap_or(false);
                warn!(
                    "QQ gateway: invalid session (resumable={resumable}, was_resuming={resuming})"
                );
                if !resumable {
                    *session_state.lock().await = None;
                    session_invalid = true;
                }
                graceful = true;
                break;
            }
            11 => {
                debug!("QQ gateway: heartbeat ack");
            }
            other => {
                debug!("QQ gateway: ignoring op {other}");
            }
        }
    }

    heartbeat_handle.abort();
    if let Ok(mut w) = Arc::try_unwrap(writer).map(|m| m.into_inner()) {
        let _ = w.send(Message::Close(None)).await;
        let _ = w.close().await;
    }

    if session_invalid {
        return Ok(false);
    }
    Ok(graceful)
}

/// Persisted across reconnects to drive Resume.
#[derive(Debug, Default, Clone)]
pub struct SessionState {
    pub session_id: String,
    pub last_seq: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("decode: {0}")]
    Decode(String),
    #[error("connection closed")]
    Closed,
}

async fn next_payload(
    read: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
) -> Result<Value, GatewayError> {
    loop {
        let msg = match read.next().await {
            Some(Ok(m)) => m,
            Some(Err(e)) => return Err(GatewayError::Transport(e.to_string())),
            None => return Err(GatewayError::Closed),
        };
        match msg {
            Message::Text(t) => {
                return serde_json::from_str::<Value>(&t)
                    .map_err(|e| GatewayError::Decode(e.to_string()));
            }
            Message::Binary(b) => {
                return serde_json::from_slice::<Value>(&b)
                    .map_err(|e| GatewayError::Decode(e.to_string()));
            }
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => return Err(GatewayError::Closed),
            Message::Frame(_) => continue,
        }
    }
}

async fn send_json<W>(writer: &mut W, value: &Value) -> Result<(), GatewayError>
where
    W: SinkExt<Message> + Unpin,
    <W as futures_util::Sink<Message>>::Error: std::fmt::Display,
{
    writer
        .send(Message::Text(value.to_string()))
        .await
        .map_err(|e| GatewayError::Transport(format!("send: {e}")))
}

async fn dispatch_event(
    t: &str,
    d: Value,
    session_state: &Arc<Mutex<Option<SessionState>>>,
    tx: &mpsc::Sender<GatewayEvent>,
) -> Result<(), GatewayError> {
    match t {
        "READY" => {
            #[derive(Deserialize)]
            struct Ready {
                #[serde(default)]
                session_id: String,
                #[serde(default)]
                user: ReadyUser,
            }
            #[derive(Default, Deserialize)]
            struct ReadyUser {
                #[serde(default)]
                username: String,
            }
            let ready: Ready =
                serde_json::from_value(d).map_err(|e| GatewayError::Decode(e.to_string()))?;
            {
                let mut s = session_state.lock().await;
                let entry = s.get_or_insert_with(SessionState::default);
                entry.session_id = ready.session_id.clone();
            }
            let _ = tx
                .send(GatewayEvent::Ready {
                    session_id: ready.session_id,
                    bot_name: ready.user.username,
                })
                .await;
        }
        "RESUMED" => {
            info!("QQ gateway: resumed");
        }
        "C2C_MESSAGE_CREATE" => {
            let _ = tx.send(GatewayEvent::C2cMessage(d)).await;
        }
        "GROUP_AT_MESSAGE_CREATE" => {
            let _ = tx.send(GatewayEvent::GroupAtMessage(d)).await;
        }
        "DIRECT_MESSAGE_CREATE" => {
            let _ = tx.send(GatewayEvent::DirectMessage(d)).await;
        }
        other => {
            debug!("QQ gateway: ignoring event {other}");
        }
    }
    Ok(())
}

/// Top-level reconnect loop. Returns when `running` flips to false.
pub async fn run_with_reconnect(
    fetch_ws_url: impl Fn() -> futures_util::future::BoxFuture<'static, Result<String, String>>
    + Send
    + Sync
    + 'static,
    fetch_auth: impl Fn() -> futures_util::future::BoxFuture<'static, Result<GatewayAuth, String>>
    + Send
    + Sync
    + 'static,
    running: Arc<AtomicBool>,
    tx: mpsc::Sender<GatewayEvent>,
) {
    let session_state: Arc<Mutex<Option<SessionState>>> = Arc::new(Mutex::new(None));

    while running.load(Ordering::SeqCst) {
        let url = match fetch_ws_url().await {
            Ok(u) => u,
            Err(e) => {
                error!("QQ gateway: fetch ws url failed: {e}");
                sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        let auth = match fetch_auth().await {
            Ok(a) => a,
            Err(e) => {
                error!("QQ gateway: fetch auth failed: {e}");
                sleep(Duration::from_secs(5)).await;
                continue;
            }
        };
        match run_session(
            &url,
            &auth,
            session_state.clone(),
            running.clone(),
            tx.clone(),
        )
        .await
        {
            Ok(_) => {
                if !running.load(Ordering::SeqCst) {
                    break;
                }
                info!("QQ gateway: session ended, reconnecting in 5s");
                sleep(Duration::from_secs(5)).await;
            }
            Err(e) => {
                warn!("QQ gateway: session error: {e}");
                if !running.load(Ordering::SeqCst) {
                    break;
                }
                sleep(Duration::from_secs(5)).await;
            }
        }
    }
}
