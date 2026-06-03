//! Example Plugin: weather lookup
//!
//! Demonstrates:
//!   - Agent Runtime host_call ABI (network.http capability)
//!   - AgentContext integration: writing results as extensions
//!
//! Build:
//!   cargo build --target wasm32-unknown-unknown --release
//!   # output: target/wasm32-unknown-unknown/release/weather_plugin.wasm

use extism_pdk::*;
use serde::{Deserialize, Serialize};

/// Args for the `lookup` export
///
/// `_agent_context` is automatically injected by the orchestrator;
/// the plugin can read it to access previous tool results, entities, etc.
#[derive(Debug, Deserialize)]
struct LookupArgs {
    /// City name to query (e.g. "Beijing", "Tokyo")
    city: String,

    /// AgentContext snapshot injected by infrastructure (read-only)
    #[serde(default)]
    _agent_context: Option<serde_json::Value>,
}

/// Result returned to the orchestrator / LLM
///
/// Includes `_agent_context_updates` so the infrastructure can
/// automatically persist the weather card as an AgentContext extension.
#[derive(Debug, Serialize)]
struct LookupResult {
    city: String,
    temp_c: f64,
    summary: String,

    /// AgentContext updates: weather result as a card extension
    #[serde(skip_serializing_if = "Option::is_none")]
    _agent_context_updates: Option<AgentContextUpdates>,
}

/// Updates to write back to AgentContext after execution.
#[derive(Debug, Serialize)]
struct AgentContextUpdates {
    extensions: Vec<ExtensionEntry>,
}

/// A single extension entry to add to AgentContext.
#[derive(Debug, Serialize)]
struct ExtensionEntry {
    /// Unique identifier for this extension
    id: String,
    /// Extension type: "card", "image", "suggestion", "link", "button", "table", "chart"
    content_type: String,
    /// Structured data for this extension
    data: serde_json::Value,
}

/// Host call envelope — must match contracts/host-functions.md §2
#[derive(Debug, Serialize)]
struct HostCallEnvelope<'a, T: Serialize> {
    capability: &'a str,
    args: T,
}

#[derive(Debug, Deserialize)]
struct HostCallReply<T> {
    ok: bool,
    #[serde(default)]
    data: Option<T>,
    #[serde(default)]
    code: Option<u16>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Serialize)]
struct HttpArgs<'a> {
    method: &'a str,
    url: String,
}

#[derive(Debug, Default, Deserialize)]
struct HttpReply {
    status: u16,
    body: String,
    #[serde(default)]
    body_truncated: bool,
}

// ---------- Plugin entry ----------

#[plugin_fn]
pub fn lookup(args_json: String) -> FnResult<String> {
    let args: LookupArgs = serde_json::from_str(&args_json)
        .map_err(|e| Error::msg(format!("invalid args: {e}")))?;

    // ★ Read AgentContext: check if we already have weather data
    //    for this city in previous tool results (e.g., cache hit).
    if let Some(ref ctx) = args._agent_context {
        if let Some(cached) = find_cached_weather(ctx, &args.city) {
            let out = LookupResult {
                city: args.city.clone(),
                temp_c: cached.temp_c,
                summary: cached.summary.clone(),
                _agent_context_updates: Some(AgentContextUpdates {
                    extensions: vec![weather_card(&args.city, cached.temp_c, &cached.summary)],
                }),
            };
            return Ok(serde_json::to_string(&out)?);
        }
    }

    // Build the host_call envelope to fetch a weather feed.
    // wttr.in returns plain text by default; format=j1 gives JSON.
    let envelope = HostCallEnvelope {
        capability: "network.http",
        args: HttpArgs {
            method: "GET",
            url: format!("https://wttr.in/{}?format=j1", args.city),
        },
    };
    let envelope_json = serde_json::to_string(&envelope)?;

    let resp_str = unsafe { host_call(envelope_json)? };
    let resp: HostCallReply<HttpReply> = serde_json::from_str(&resp_str)
        .map_err(|e| Error::msg(format!("invalid host reply: {e}")))?;

    if !resp.ok {
        return Err(Error::msg(format!(
            "network.http failed: code={:?} message={:?}",
            resp.code, resp.message
        ))
        .into());
    }
    let http = resp.data.ok_or_else(|| Error::msg("missing data"))?;
    if http.status >= 400 {
        return Err(Error::msg(format!("upstream HTTP {}", http.status)).into());
    }

    // Naive parse: pull first temp + weather description from wttr JSON.
    let parsed: serde_json::Value = serde_json::from_str(&http.body)
        .map_err(|e| Error::msg(format!("upstream body parse: {e}")))?;
    let temp_c = parsed
        .pointer("/current_condition/0/temp_C")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(f64::NAN);
    let summary = parsed
        .pointer("/current_condition/0/weatherDesc/0/value")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let out = LookupResult {
        city: args.city.clone(),
        temp_c,
        summary: summary.clone(),
        _agent_context_updates: Some(AgentContextUpdates {
            extensions: vec![weather_card(&args.city, temp_c, &summary)],
        }),
    };
    Ok(serde_json::to_string(&out)?)
}

/// Build a weather card extension entry.
fn weather_card(city: &str, temp: f64, condition: &str) -> ExtensionEntry {
    ExtensionEntry {
        id: format!("weather_card_{}", city.to_lowercase()),
        content_type: "card".into(),
        data: serde_json::json!({
            "title": format!("{} 天气", city),
            "temperature": format!("{}°C", temp),
            "condition": condition,
            "icon": weather_icon(condition),
        }),
    }
}

/// Search the AgentContext snapshot for a cached weather result for the given city.
///
/// Looks through `tool_results` entries where the key starts with "weather_"
/// and the value contains the target city.
#[derive(Debug, Deserialize)]
struct CachedWeather {
    temp_c: f64,
    summary: String,
}

fn find_cached_weather(ctx: &serde_json::Value, city: &str) -> Option<CachedWeather> {
    let tool_results = ctx.get("tool_results")?.as_array()?;
    for entry in tool_results {
        let key = entry.get("key")?.as_str()?;
        let value = entry.get("value")?;

        // Match: key like "weather_weathered_Beijing" containing the city
        if key.starts_with("weather_") && key.to_lowercase().contains(&city.to_lowercase()) {
            // Try to extract cached temperature & summary
            if let (Some(temp_c), Some(summary)) = (
                value.get("temp_c").and_then(|v| v.as_f64()),
                value.get("summary").and_then(|v| v.as_str()),
            ) {
                return Some(CachedWeather {
                    temp_c,
                    summary: summary.to_string(),
                });
            }
        }
    }
    None
}

/// Map weather description to a simple emoji icon.
fn weather_icon(summary: &str) -> &'static str {
    let lower = summary.to_lowercase();
    if lower.contains("sun") || lower.contains("clear") {
        "☀️"
    } else if lower.contains("cloud") || lower.contains("overcast") {
        "☁️"
    } else if lower.contains("rain") || lower.contains("drizzle") {
        "🌧️"
    } else if lower.contains("snow") {
        "❄️"
    } else if lower.contains("fog") || lower.contains("mist") {
        "🌫️"
    } else {
        "🌤️"
    }
}

// The host_call host function (provided by the host runtime).
#[host_fn]
extern "ExtismHost" {
    fn host_call(envelope: String) -> String;
}
