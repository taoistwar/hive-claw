//! Runtime audit log — emits structured tracing events.
//!
//! All host_call / plugin_invoke / workflow_node / agent_route / llm_invoke /
//! llm_fallback audit events are logged via tracing for collection by the
//! observability stack (no longer written to a database table).

use serde_json::Value;

/// 单条 audit 记录的参数集
#[derive(Debug, Clone)]
pub struct AuditRecord<'a> {
    pub request_id: Option<&'a str>,
    pub session_id: Option<i64>,
    pub agent_id: Option<i64>,
    pub plugin_id: Option<i64>,
    pub function_id: Option<i64>,
    pub capability: Option<&'a str>,
    /// e.g. capability_call / capability_denied / plugin_invoke / workflow_node /
    /// agent_route / llm_invoke / llm_fallback
    pub event_type: &'a str,
    /// success / error / denied / timeout
    pub outcome: &'a str,
    pub elapsed_ms: Option<i32>,
    pub error_message: Option<&'a str>,
    pub payload_summary: Option<Value>,
}

/// Emit a structured tracing event — audit failures no longer interact with the
/// database and cannot block the main flow.
pub fn record(rec: AuditRecord<'_>) {
    tracing::info!(
        request_id = rec.request_id,
        session_id = rec.session_id,
        agent_id = rec.agent_id,
        plugin_id = rec.plugin_id,
        function_id = rec.function_id,
        capability = rec.capability,
        event_type = rec.event_type,
        outcome = rec.outcome,
        elapsed_ms = rec.elapsed_ms,
        error_message = rec.error_message,
        payload_summary = ?rec.payload_summary,
        "runtime_audit",
    );
}

/// 简易脱敏：对 `args` 中常见敏感字段做 mask
pub fn redact_args(args: &Value) -> Value {
    fn walk(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let mut out = serde_json::Map::with_capacity(m.len());
                for (k, val) in m {
                    let key_lower = k.to_lowercase();
                    let masked = if key_lower.contains("password")
                        || key_lower.contains("token")
                        || key_lower.contains("secret")
                        || key_lower.contains("api_key")
                    {
                        Value::String("***".into())
                    } else {
                        walk(val)
                    };
                    out.insert(k.clone(), masked);
                }
                Value::Object(out)
            }
            Value::Array(arr) => Value::Array(arr.iter().map(walk).collect()),
            other => other.clone(),
        }
    }
    walk(args)
}
