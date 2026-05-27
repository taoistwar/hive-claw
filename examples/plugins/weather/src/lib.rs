//! Example Plugin: weather lookup
//!
//! Demonstrates the Agent Runtime host_call ABI:
//!   - Export `lookup(args_json: String) -> String`
//!   - Call host_call(envelope_json) to use `network.http` capability
//!   - Parse response + return result JSON
//!
//! Build:
//!   cargo build --target wasm32-unknown-unknown --release
//!   # output: target/wasm32-unknown-unknown/release/weather_plugin.wasm
//!
//! Upload via management UI or curl (see examples/plugins/README.md).

use extism_pdk::*;
use serde::{Deserialize, Serialize};

/// Args for the `lookup` export
#[derive(Debug, Deserialize)]
struct LookupArgs {
    /// City name to query (e.g. "Beijing", "Tokyo")
    city: String,
}

/// Result returned to the orchestrator / LLM
#[derive(Debug, Serialize)]
struct LookupResult {
    city: String,
    temp_c: f64,
    summary: String,
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
        city: args.city,
        temp_c,
        summary,
    };
    Ok(serde_json::to_string(&out)?)
}

// The host_call host function (provided by the host runtime).
#[host_fn]
extern "ExtismHost" {
    fn host_call(envelope: String) -> String;
}
