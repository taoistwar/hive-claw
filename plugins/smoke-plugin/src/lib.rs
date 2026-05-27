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

#[host_fn]
extern "ExtismHost" {
    fn host_call(envelope: String) -> String;
}

#[derive(Serialize)]
struct CallRequest<'a> {
    capability: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    args: Option<serde_json::Value>,
}

#[derive(Deserialize, Debug)]
struct CallResponse<T> {
    ok: bool,
    #[serde(default)]
    code: Option<u16>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    data: Option<T>,
}

fn call<T: for<'de> Deserialize<'de> + Default>(cap: &str, args: Option<serde_json::Value>) -> FnResult<CallResponse<T>> {
    let req = CallRequest { capability: cap, args };
    let envelope = serde_json::to_string(&req)?;
    let raw = unsafe { host_call(envelope)? };
    let resp: CallResponse<T> = serde_json::from_str(&raw)?;
    if !resp.ok {
        let code = resp.code.unwrap_or(1);
        let msg = resp.message.unwrap_or_else(|| "unknown error".into());
        return Err(Error::msg(format!("{}: {}", code, msg)).into());
    }
    Ok(resp)
}

#[plugin_fn]
pub fn ping(_: String) -> FnResult<String> {
    let resp: CallResponse<TimeNowData> = call("time.now", None)?;
    let data = resp.data.unwrap();
    let out = serde_json::json!({
        "status": "ok",
        "unix_ms": data.unix_ms,
        "iso": data.iso,
    });
    Ok(serde_json::to_string(&out)?)
}

#[plugin_fn]
pub fn echo(input: String) -> FnResult<String> {
    let log_args = serde_json::json!({
        "level": "info",
        "message": format!("smoke-plugin echo: {input}"),
        "fields": { "source": "smoke-plugin" },
    });
    let _log_resp: CallResponse<LogEmitData> = call("log.emit", Some(log_args))?;
    let out = serde_json::json!({
        "echo": input,
        "logged": true,
    });
    Ok(serde_json::to_string(&out)?)
}

#[plugin_fn]
pub fn http_get(input: String) -> FnResult<String> {
    let http_args = serde_json::json!({
        "method": "GET",
        "url": input,
        "timeout_ms": 5000,
    });
    let resp: CallResponse<HttpData> = call("network.http", Some(http_args))?;
    let data = resp.data.unwrap();
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
    let write_args = serde_json::json!({
        "path": path,
        "content_base64": BASE64.encode(input.as_bytes()),
    });
    let _write_resp: CallResponse<FsWriteData> = call("fs.write", Some(write_args))?;
    let read_args = serde_json::json!({ "path": path });
    let read_resp: CallResponse<FsReadData> = call("fs.read", Some(read_args))?;
    let read_data = read_resp.data.unwrap();
    let content = String::from_utf8_lossy(&BASE64.decode(&read_data.content_base64).unwrap_or_default()).into_owned();
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

    match call::<TimeNowData>("time.now", None) {
        Ok(r) => {
            let d = r.data.unwrap();
            results.insert("time".into(), serde_json::json!({ "ok": true, "unix_ms": d.unix_ms }));
        }
        Err(e) => {
            results.insert("time".into(), serde_json::json!({ "ok": false, "error": format!("{e:?}") }));
        }
    }

    let log_args = serde_json::json!({
        "level": "info",
        "message": format!("full_demo called with: {input}"),
    });
    match call::<LogEmitData>("log.emit", Some(log_args)) {
        Ok(_) => { results.insert("log".into(), serde_json::json!({ "ok": true })); }
        Err(e) => { results.insert("log".into(), serde_json::json!({ "ok": false, "error": format!("{e:?}") })); }
    }

    let path = "/tmp/plugin/demo.txt";
    let write_args = serde_json::json!({
        "path": path,
        "content_base64": BASE64.encode(input.as_bytes()),
    });
    match call::<FsWriteData>("fs.write", Some(write_args)) {
        Ok(wr) => {
            let read_args = serde_json::json!({ "path": path });
            match call::<FsReadData>("fs.read", Some(read_args)) {
                Ok(rr) => {
                    let rd = rr.data.unwrap();
                    let content = String::from_utf8_lossy(&BASE64.decode(&rd.content_base64).unwrap_or_default()).into_owned();
                    results.insert("fs".into(), serde_json::json!({
                        "ok": true,
                        "written": wr.data.map(|d| d.bytes_written),
                        "read_match": content == input,
                    }));
                }
                Err(e) => { results.insert("fs".into(), serde_json::json!({ "ok": false, "error": format!("{e:?}") })); }
            }
        }
        Err(e) => { results.insert("fs".into(), serde_json::json!({ "ok": false, "error": format!("{e:?}") })); }
    }

    let out = serde_json::Value::Object(results);
    Ok(serde_json::to_string(&out)?)
}

#[derive(Deserialize, Debug, Default)]
struct TimeNowData {
    #[serde(default)]
    unix_ms: i64,
    #[serde(default)]
    iso: String,
}

#[derive(Deserialize, Debug, Default)]
struct LogEmitData {
    #[serde(default)]
    logged: bool,
}

#[derive(Deserialize, Debug, Default)]
struct HttpData {
    #[serde(default)]
    status: u16,
    #[serde(default)]
    headers: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    body: String,
}

#[derive(Deserialize, Debug, Default)]
struct FsReadData {
    #[serde(default)]
    content_base64: String,
}

#[derive(Deserialize, Debug, Default)]
struct FsWriteData {
    #[serde(default)]
    bytes_written: usize,
}
