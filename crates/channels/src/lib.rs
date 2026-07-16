//! Chat channel implementations and the [`ChannelManager`] that
//! coordinates them. Rust port of `nanobot.channels`.
//!
//! Public surface mirrors the Python module:
//!
//! * [`base::Channel`] — async trait every channel implements.
//! * [`manager::ChannelManager`] — discovers, starts and stops enabled
//!   channels, plus dispatches outbound messages with retry.
//! * [`weixin::WeixinChannel`] — personal WeChat (HTTP long-poll).
//! * [`qq::QQChannel`] — QQ Bot Open Platform (REST + WebSocket gateway).

pub mod base;
pub mod dingtalk;
pub mod discord;
pub mod email;
pub mod feishu;
pub mod manager;
pub mod matrix;
pub mod mochat;
pub mod msteams;
pub mod qq;
pub mod qq_gateway;
pub mod registry;
pub mod slack;
pub mod telegram;
pub mod websocket;
pub mod wecom;
pub mod weixin;
pub mod whatsapp;

pub use base::{
    Channel, ChannelError, ChannelInfo, ChannelResult, TranscriptionSettings, handle_inbound,
    is_allowed, transcribe_audio,
};
pub use manager::{ChannelManager, ChannelStatus};
pub use qq::{QQChannel, QQConfig};
pub use registry::{ChannelEntry, build_enabled_channels, build_one, known_channel_names};
pub use weixin::{WeixinChannel, WeixinConfig};
