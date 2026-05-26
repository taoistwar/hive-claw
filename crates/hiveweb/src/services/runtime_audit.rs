//! Runtime audit log service (T105 / US4)
//!
//! 写 `runtime_audit_logs` 表；事件类型见 V017 注释。
//! 所有 host_call / plugin_invoke / workflow_node / agent_route / llm_invoke /
//! llm_fallback 都通过本 service 落表。

use serde_json::Value;
use sqlx::MySqlPool;

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

/// best-effort 写入；audit 失败不阻断主流程，但会 warn log。
pub async fn record(pool: &MySqlPool, rec: AuditRecord<'_>) {
    let res = sqlx::query(
        r#"INSERT INTO runtime_audit_logs
           (request_id, session_id, agent_id, plugin_id, function_id,
            capability, event_type, outcome, elapsed_ms, error_message, payload_summary)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(rec.request_id)
    .bind(rec.session_id)
    .bind(rec.agent_id)
    .bind(rec.plugin_id)
    .bind(rec.function_id)
    .bind(rec.capability)
    .bind(rec.event_type)
    .bind(rec.outcome)
    .bind(rec.elapsed_ms)
    .bind(rec.error_message.map(|s| {
        // 截断到 512 字符以适配 VARCHAR(512)
        if s.len() <= 512 {
            s.to_string()
        } else {
            s.chars().take(512).collect()
        }
    }))
    .bind(&rec.payload_summary)
    .execute(pool)
    .await;
    if let Err(e) = res {
        tracing::warn!(error = %e, "runtime_audit insert failed");
    }
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
