//! Utility capabilities — time.now / log.emit (T101 / US4)

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize)]
pub struct TimeNowReply {
    /// Unix 毫秒
    pub unix_ms: i64,
    /// RFC3339 UTC
    pub iso: String,
}

pub fn time_now() -> Value {
    let now = chrono::Utc::now();
    serde_json::to_value(TimeNowReply {
        unix_ms: now.timestamp_millis(),
        iso: now.to_rfc3339(),
    })
    .unwrap_or(Value::Null)
}

#[derive(Debug, Deserialize)]
pub struct LogEmitArgs {
    pub level: Option<String>,
    pub message: String,
    #[serde(default)]
    pub fields: Option<Value>,
}

/// log.emit — 把 Plugin 侧的结构化日志透传到宿主 tracing。
/// rate-limit 在 dispatcher 入口处统一做（log.emit 没有特殊配额，与其它 capability 共享 host_call 整体并发）。
pub fn log_emit(args: LogEmitArgs, plugin_id: Option<i64>, agent_id: Option<i64>) -> Value {
    let level = args.level.as_deref().unwrap_or("info").to_ascii_lowercase();
    match level.as_str() {
        "error" => tracing::error!(plugin_id, agent_id, fields = ?args.fields, "{}", args.message),
        "warn" => tracing::warn!(plugin_id, agent_id, fields = ?args.fields, "{}", args.message),
        "debug" => tracing::debug!(plugin_id, agent_id, fields = ?args.fields, "{}", args.message),
        _ => tracing::info!(plugin_id, agent_id, fields = ?args.fields, "{}", args.message),
    }
    Value::Object(serde_json::Map::new())
}
