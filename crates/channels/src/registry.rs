//! Channel registry — currently a static map from channel name to
//! constructor. The Python original walks `pkgutil` plus entry-points;
//! the Rust port keeps the same shape but enumerates known channels by
//! hand (qq, weixin) since Rust has no equivalent of dynamic plugin
//! discovery without third-party crates.

use std::sync::Arc;

use serde_json::Value;

use bus::MessageBus;
use config::schema::Config;

use crate::base::{Channel, TranscriptionSettings};

/// One row produced by [`build_enabled_channels`].
pub struct ChannelEntry {
    pub name: String,
    pub display_name: String,
    pub channel: Arc<dyn Channel>,
}

/// Iterate over the static channel list and build the ones that are
/// enabled in the config (`channels.<name>.enabled = true`).
pub fn build_enabled_channels(
    config: &Config,
    bus: &MessageBus,
    transcription: &TranscriptionSettings,
) -> Result<Vec<ChannelEntry>, String> {
    let mut out: Vec<ChannelEntry> = Vec::new();
    for (name, ctor) in known_channels() {
        let Some(section) = config.channels.extras.get(*name) else {
            continue;
        };
        if !is_enabled(section) {
            continue;
        }
        match ctor(section.clone(), bus.clone(), transcription.clone()) {
            Ok(entry) => out.push(entry),
            Err(e) => {
                log::warn!("{name} channel not available: {e}");
            }
        }
    }
    Ok(out)
}

fn is_enabled(section: &Value) -> bool {
    section
        .get("enabled")
        .map(|v| match v {
            Value::Bool(b) => *b,
            Value::String(s) => s == "true" || s == "1",
            Value::Number(n) => n.as_i64().map(|i| i != 0).unwrap_or(false),
            _ => false,
        })
        .unwrap_or(false)
}

type ChannelCtor =
    fn(Value, MessageBus, TranscriptionSettings) -> Result<ChannelEntry, String>;

fn known_channels() -> &'static [(&'static str, ChannelCtor)] {
    &[
        ("weixin", crate::weixin::build),
        ("qq", crate::qq::build),
    ]
}

/// Names of every built-in channel (regardless of whether it's enabled
/// in the user's config). Mirrors `discover_channel_names` in the
/// Python registry.
pub fn known_channel_names() -> Vec<&'static str> {
    known_channels().iter().map(|(n, _)| *n).collect()
}

/// Build a single channel by name, ignoring the `enabled` flag. Returns
/// `Err` when the channel name is unknown or the constructor itself
/// fails (bad config, missing credentials, etc.). Used by the CLI
/// `channels login` command which needs a channel instance even when
/// it isn't enabled yet.
pub fn build_one(
    name: &str,
    section: Value,
    bus: MessageBus,
    transcription: TranscriptionSettings,
) -> Result<ChannelEntry, String> {
    for (known, ctor) in known_channels() {
        if *known == name {
            return ctor(section, bus, transcription);
        }
    }
    let available: Vec<&str> = known_channels().iter().map(|(n, _)| *n).collect();
    Err(format!(
        "Unknown channel: {name}  Available: {}",
        available.join(", ")
    ))
}

/// Returns default configuration values for all known channels.
/// Useful for generating initial config files or merging with user overrides.
pub fn default_config() -> Value {
    let mut map = serde_json::Map::new();
    for (name, _) in known_channels() {
        map.insert(
            name.to_string(),
            serde_json::json!({
                "enabled": false,
            }),
        );
    }
    Value::Object(map)
}
