//! `nanobot status`, `nanobot channels …`, `nanobot plugins …`.
//!
//! These are read-only commands that report on the current
//! configuration. They share the runtime helpers in [`crate::runtime`].

use std::path::PathBuf;
use std::sync::Arc;

use bus::MessageBus;
use channels::base::TranscriptionSettings;
use channels::{build_one, known_channel_names};
use config::Config;
use config::paths::get_workspace_path;
use providers::PROVIDERS;
use serde_json::Value;

use crate::runtime::{Runtime};

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

pub async fn status(workspace: Option<PathBuf>, config: Option<PathBuf>) -> Result<(), String> {
    let Runtime {
        config: cfg,
        config_path,
    } = Runtime::from_config(config.as_deref(), workspace.as_deref())?;

    let workspace = get_workspace_path(Some(&cfg.agents.defaults.workspace));
    let cfg_disp = config_path
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| config::get_config_path().display().to_string());

    println!("nanobot status");
    println!("──────────────");
    println!("Config:    {cfg_disp}");
    println!("Workspace: {}", workspace.display());
    println!("Model:     {}", cfg.agents.defaults.model);
    println!(
        "Provider:  {}",
        match cfg.agents.defaults.provider.as_str() {
            "" | "auto" => "auto".to_string(),
            other => other.to_string(),
        }
    );
    println!();
    println!("Provider credentials:");
    for spec in PROVIDERS {
        if spec.name == "custom" {
            continue;
        }
        let pc = providers::provider_config_for(&cfg, Some(spec));
        let has_key = pc
            .and_then(|p| p.api_key.as_deref())
            .map(|k| !k.is_empty())
            .unwrap_or(false);
        let env_present = !spec.env_key.is_empty()
            && std::env::var(spec.env_key)
                .map(|v| !v.is_empty())
                .unwrap_or(false);
        let mark = if has_key {
            "✓ configured"
        } else if env_present {
            "✓ env"
        } else if spec.is_oauth {
            "  OAuth (run `nanobot provider login`)"
        } else if spec.is_local {
            "  local"
        } else if spec.is_direct {
            "  direct"
        } else {
            "  not set"
        };
        println!("  {:<22} {}", spec.label(), mark);
    }

    if let Some(spec) = providers::resolve_spec(&cfg) {
        println!();
        println!("Selected provider for this model: {}", spec.label());
    } else {
        println!();
        println!(
            "Selected provider for this model: <none auto-detected>; will fall back to OpenAI-compat."
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// channels
// ---------------------------------------------------------------------------

pub async fn channels_status(config: Option<PathBuf>) -> Result<(), String> {
    let Runtime { config: cfg, .. } = Runtime::from_config(config.as_deref(), None)?;
    println!("Channels");
    println!("────────");
    let configured = list_channels(&cfg);
    if configured.is_empty() {
        println!("  (none configured)");
    } else {
        let builtin = known_channel_names();
        for (name, enabled) in configured {
            let kind = if builtin.iter().any(|n| *n == name) {
                "builtin"
            } else {
                "plugin"
            };
            let mark = if enabled { "enabled " } else { "disabled" };
            println!("  {mark}  {name:<10} ({kind})");
        }
    }
    Ok(())
}

/// Trigger the channel's interactive login (e.g. WeChat QR scan).
///
/// Builds a single channel instance via the registry — the channel
/// doesn't need to be enabled in the config beforehand, but its
/// section must contain enough info to construct it.
pub async fn channels_login(
    name: String,
    force: bool,
    config: Option<PathBuf>,
) -> Result<(), String> {
    let Runtime { config: cfg, .. } = Runtime::from_config(config.as_deref(), None)?;

    let section = cfg
        .channels
        .extras
        .get(&name)
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));

    // The login flow only needs the bus to satisfy the Channel trait;
    // nothing actually publishes during the login dance.
    let bus = MessageBus::new();
    let transcription = TranscriptionSettings::default();
    let entry = build_one(&name, section, bus, transcription)?;

    println!("{} login starting…", entry.display_name);
    let channel = entry.channel;
    let success = Arc::clone(&channel)
        .login(force)
        .await
        .map_err(|e| format!("{name} login failed: {e}"))?;

    if success {
        println!("{} login complete.", entry.display_name);
        Ok(())
    } else {
        Err(format!(
            "{} login was cancelled or failed",
            entry.display_name
        ))
    }
}

// ---------------------------------------------------------------------------
// plugins list
// ---------------------------------------------------------------------------

pub async fn plugins_list(config: Option<PathBuf>) -> Result<(), String> {
    let Runtime { config: cfg, .. } = Runtime::from_config(config.as_deref(), None)?;
    let builtin = known_channel_names();
    let configured = list_channels(&cfg);

    let mut all: Vec<(String, bool, &'static str)> = Vec::new();
    for name in &builtin {
        let enabled = configured
            .iter()
            .find(|(n, _)| n == *name)
            .map(|(_, e)| *e)
            .unwrap_or(false);
        all.push(((*name).to_string(), enabled, "builtin"));
    }
    for (name, enabled) in &configured {
        if !builtin.iter().any(|n| n == name) {
            all.push((name.clone(), *enabled, "plugin"));
        }
    }
    all.sort_by(|a, b| a.0.cmp(&b.0));

    println!("Channel plugins");
    println!("───────────────");
    if all.is_empty() {
        println!("  (none)");
    } else {
        for (name, enabled, kind) in all {
            let mark = if enabled { "[on] " } else { "[off]" };
            println!("  {mark}  {name:<12} {kind}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn list_channels(cfg: &Config) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    for (name, value) in cfg.channels.extras.iter() {
        let enabled = value
            .as_object()
            .and_then(|o| o.get("enabled"))
            .map(value_truthy)
            .unwrap_or(false);
        out.push((name.clone(), enabled));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn value_truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_i64().map(|i| i != 0).unwrap_or(false),
        Value::String(s) => !s.is_empty() && s != "false" && s != "0",
        _ => false,
    }
}
