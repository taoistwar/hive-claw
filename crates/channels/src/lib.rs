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
pub mod manager;
pub mod qq;
pub mod qq_gateway;
pub mod registry;
pub mod weixin;

pub use base::{
    handle_inbound, is_allowed, transcribe_audio, Channel, ChannelError, ChannelResult,
    ChannelInfo, TranscriptionSettings,
};
pub use manager::{ChannelManager, ChannelStatus};
pub use registry::{build_enabled_channels, build_one, known_channel_names, ChannelEntry};
pub use qq::{QQChannel, QQConfig};
pub use weixin::{WeixinChannel, WeixinConfig};
