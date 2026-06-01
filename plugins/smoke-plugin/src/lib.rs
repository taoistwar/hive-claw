//! Smoke Plugin — WASM smoke test for hive-claw Agent Runtime.
//!
//! Demonstrates the capability-based `host_call` ABI by exercising
//! several capabilities: `time.now`, `log.emit`, `network.http`, `fs.read/write`, etc.
//!
//! Compile:
//!   cargo build --target wasm32-unknown-unknown --release -p smoke-plugin
//!   cp target/wasm32-unknown-unknown/release/smoke_plugin.wasm .

use extism_pdk::*;
use serde::{Deserialize, Serialize};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

// ---------- Host call envelope (matches contracts/host-functions.md §2) ----------

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

// ---------- Capability-specific args / reply data ----------

#[derive(Debug, Deserialize, Default)]
struct TimeNowData {
    #[serde(default)]
    unix_ms: i64,
    #[serde(default)]
    iso: String,
}

#[derive(Debug, Deserialize, Default)]
struct LogEmitData {
    #[serde(default)]
    logged: bool,
}

#[derive(Debug, Serialize)]
struct LogEmitArgs<'a> {
    level: &'a str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    fields: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Deserialize, Default)]
struct HttpData {
    #[serde(default)]
    status: u16,
    #[serde(default)]
    headers: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    body: String,
}

#[derive(Debug, Serialize)]
struct HttpGetArgs<'a> {
    method: &'a str,
    url: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout_ms: Option<u32>,
}

#[derive(Debug, Deserialize, Default)]
struct FsReadData {
    #[serde(default)]
    content_base64: String,
}

#[derive(Debug, Serialize)]
struct FsReadArgs<'a> {
    path: &'a str,
}

#[derive(Debug, Deserialize, Default)]
struct FsWriteData {
    #[serde(default)]
    bytes_written: usize,
}

#[derive(Debug, Serialize)]
struct FsWriteArgs<'a> {
    path: &'a str,
    content_base64: String,
}

// ---------- Plugin exports ----------

#[plugin_fn]
pub fn ping(_: String) -> FnResult<String> {
    let envelope = HostCallEnvelope {
        capability: "time.now",
        args: serde_json::Value::Null,
    };
    let envelope_json = serde_json::to_string(&envelope)?;
    let resp_str = unsafe { host_call(envelope_json)? };
    let resp: HostCallReply<TimeNowData> = serde_json::from_str(&resp_str)
        .map_err(|e| Error::msg(format!("invalid host reply: {e}")))?;

    if !resp.ok {
        return Err(Error::msg(format!(
            "time.now failed: code={:?} message={:?}",
            resp.code, resp.message
        ))
        .into());
    }
    let data = resp.data.unwrap_or_default();
    let out = serde_json::json!({
        "status": "ok",
        "unix_ms": data.unix_ms,
        "iso": data.iso,
    });
    Ok(serde_json::to_string(&out)?)
}

#[plugin_fn]
pub fn echo(input: String) -> FnResult<String> {
    let envelope = HostCallEnvelope {
        capability: "log.emit",
        args: LogEmitArgs {
            level: "info",
            message: format!("smoke-plugin echo: {input}"),
            fields: {
                let mut m = serde_json::Map::new();
                m.insert("source".into(), serde_json::Value::String("smoke-plugin".into()));
                Some(m)
            },
        },
    };
    let envelope_json = serde_json::to_string(&envelope)?;
    let resp_str = unsafe { host_call(envelope_json)? };
    let _resp: HostCallReply<LogEmitData> = serde_json::from_str(&resp_str)
        .map_err(|e| Error::msg(format!("invalid host reply: {e}")))?;

    let out = serde_json::json!({
        "echo": input,
        "logged": true,
    });
    Ok(serde_json::to_string(&out)?)
}

#[plugin_fn]
pub fn http_get(input: String) -> FnResult<String> {
    let envelope = HostCallEnvelope {
        capability: "network.http",
        args: HttpGetArgs {
            method: "GET",
            url: &input,
            timeout_ms: Some(5000),
        },
    };
    let envelope_json = serde_json::to_string(&envelope)?;
    let resp_str = unsafe { host_call(envelope_json)? };
    let resp: HostCallReply<HttpData> = serde_json::from_str(&resp_str)
        .map_err(|e| Error::msg(format!("invalid host reply: {e}")))?;

    if !resp.ok {
        return Err(Error::msg(format!(
            "network.http failed: code={:?} message={:?}",
            resp.code, resp.message
        ))
        .into());
    }
    let data = resp.data.unwrap_or_default();
    let out = serde_json::json!({
        "status": data.status,
        "body_len": data.body.len(),
        "content_type": data.headers.get("content-type").cloned().unwrap_or_default(),
    });
    Ok(serde_json::to_string(&out)?)
}

#[plugin_fn]
pub fn fs_roundtrip(input: String) -> FnResult<String> {
    let path = "/tmp/plugin/smoke.txt";

    // Write
    let write_envelope = HostCallEnvelope {
        capability: "fs.write",
        args: FsWriteArgs {
            path,
            content_base64: BASE64.encode(input.as_bytes()),
        },
    };
    let resp_str = unsafe { host_call(serde_json::to_string(&write_envelope)?)? };
    let _write_resp: HostCallReply<FsWriteData> = serde_json::from_str(&resp_str)
        .map_err(|e| Error::msg(format!("invalid host reply: {e}")))?;

    // Read back
    let read_envelope = HostCallEnvelope {
        capability: "fs.read",
        args: FsReadArgs { path },
    };
    let resp_str = unsafe { host_call(serde_json::to_string(&read_envelope)?)? };
    let read_resp: HostCallReply<FsReadData> = serde_json::from_str(&resp_str)
        .map_err(|e| Error::msg(format!("invalid host reply: {e}")))?;

    if !read_resp.ok {
        return Err(Error::msg(format!(
            "fs.read failed: code={:?} message={:?}",
            read_resp.code, read_resp.message
        ))
        .into());
    }
    let read_data = read_resp.data.unwrap_or_default();
    let content = String::from_utf8_lossy(
        &BASE64.decode(&read_data.content_base64).unwrap_or_default(),
    )
    .into_owned();

    let out = serde_json::json!({
        "written": input.len(),
        "read_back": content,
        "match": content == input,
    });
    Ok(serde_json::to_string(&out)?)
}

#[plugin_fn]
pub fn full_demo(input: String) -> FnResult<String> {
    let mut results = serde_json::Map::new();

    // time.now
    {
        let envelope = HostCallEnvelope {
            capability: "time.now",
            args: serde_json::Value::Null,
        };
        let envelope_json = serde_json::to_string(&envelope)?;
        match unsafe { host_call(envelope_json) } {
            Ok(resp_str) => {
                match serde_json::from_str::<HostCallReply<TimeNowData>>(&resp_str) {
                    Ok(r) if r.ok => {
                        let d = r.data.unwrap_or_default();
                        results.insert("time".into(), serde_json::json!({ "ok": true, "unix_ms": d.unix_ms }));
                    }
                    Ok(r) => {
                        results.insert("time".into(), serde_json::json!({ "ok": false, "error": format!("code={:?} msg={:?}", r.code, r.message) }));
                    }
                    Err(e) => {
                        results.insert("time".into(), serde_json::json!({ "ok": false, "error": format!("{e:?}") }));
                    }
                }
            }
            Err(e) => {
                results.insert("time".into(), serde_json::json!({ "ok": false, "error": format!("{e:?}") }));
            }
        }
    }

    // log.emit
    {
        let envelope = HostCallEnvelope {
            capability: "log.emit",
            args: LogEmitArgs {
                level: "info",
                message: format!("full_demo called with: {input}"),
                fields: None,
            },
        };
        let envelope_json = serde_json::to_string(&envelope)?;
        match unsafe { host_call(envelope_json) } {
            Ok(resp_str) => {
                match serde_json::from_str::<HostCallReply<LogEmitData>>(&resp_str) {
                    Ok(r) if r.ok => {
                        results.insert("log".into(), serde_json::json!({ "ok": true }));
                    }
                    Ok(r) => {
                        results.insert("log".into(), serde_json::json!({ "ok": false, "error": format!("code={:?} msg={:?}", r.code, r.message) }));
                    }
                    Err(e) => {
                        results.insert("log".into(), serde_json::json!({ "ok": false, "error": format!("{e:?}") }));
                    }
                }
            }
            Err(e) => {
                results.insert("log".into(), serde_json::json!({ "ok": false, "error": format!("{e:?}") }));
            }
        }
    }

    // fs.write + fs.read roundtrip
    {
        let path = "/tmp/plugin/demo.txt";
        let write_envelope = HostCallEnvelope {
            capability: "fs.write",
            args: FsWriteArgs {
                path,
                content_base64: BASE64.encode(input.as_bytes()),
            },
        };
        let envelope_json = serde_json::to_string(&write_envelope)?;
        match unsafe { host_call(envelope_json) } {
            Ok(resp_str) => {
                match serde_json::from_str::<HostCallReply<FsWriteData>>(&resp_str) {
                    Ok(wr) if wr.ok => {
                        let read_envelope = HostCallEnvelope {
                            capability: "fs.read",
                            args: FsReadArgs { path },
                        };
                        let read_json = serde_json::to_string(&read_envelope)?;
                        match unsafe { host_call(read_json) } {
                            Ok(read_str) => {
                                match serde_json::from_str::<HostCallReply<FsReadData>>(&read_str) {
                                    Ok(rr) if rr.ok => {
                                        let rd = rr.data.unwrap_or_default();
                                        let content = String::from_utf8_lossy(
                                            &BASE64.decode(&rd.content_base64).unwrap_or_default(),
                                        )
                                        .into_owned();
                                        results.insert("fs".into(), serde_json::json!({
                                            "ok": true,
                                            "written": wr.data.map(|d| d.bytes_written),
                                            "read_match": content == input,
                                        }));
                                    }
                                    Ok(rr) => {
                                        results.insert("fs".into(), serde_json::json!({ "ok": false, "error": format!("code={:?} msg={:?}", rr.code, rr.message) }));
                                    }
                                    Err(e) => {
                                        results.insert("fs".into(), serde_json::json!({ "ok": false, "error": format!("{e:?}") }));
                                    }
                                }
                            }
                            Err(e) => {
                                results.insert("fs".into(), serde_json::json!({ "ok": false, "error": format!("{e:?}") }));
                            }
                        }
                    }
                    Ok(wr) => {
                        results.insert("fs".into(), serde_json::json!({ "ok": false, "error": format!("code={:?} msg={:?}", wr.code, wr.message) }));
                    }
                    Err(e) => {
                        results.insert("fs".into(), serde_json::json!({ "ok": false, "error": format!("{e:?}") }));
                    }
                }
            }
            Err(e) => {
                results.insert("fs".into(), serde_json::json!({ "ok": false, "error": format!("{e:?}") }));
            }
        }
    }

    let out = serde_json::Value::Object(results);
    Ok(serde_json::to_string(&out)?)
}

// ---------- Host function declaration ----------

#[host_fn]
extern "ExtismHost" {
    fn host_call(envelope: String) -> String;
}

