//! Utility capabilities — time.now / log.emit (T101 / US4)

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::services::runtime_audit::{log_field_count, safe_log_level};

#[derive(Debug, Serialize)]
pub struct TimeNowReply {
    /// Unix 毫秒
    pub unix_ms: i64,
    /// RFC3339 UTC
    pub rfc3339: String,
}

pub fn time_now() -> Value {
    let now = chrono::Utc::now();
    serde_json::to_value(TimeNowReply {
        unix_ms: now.timestamp_millis(),
        rfc3339: now.to_rfc3339(),
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

/// log.emit — 只把有界元数据写入宿主 tracing，不透传 Plugin 提供的正文或字段值。
/// dispatcher 在调用本 handler 前强制 per-Plugin 100 token/s 令牌桶。
pub fn log_emit(args: LogEmitArgs, plugin_id: Option<i64>, agent_id: Option<i64>) -> Value {
    let level = safe_log_level(args.level.as_deref());
    let message_bytes = args.message.len();
    let field_count = log_field_count(args.fields.as_ref());
    match level {
        "error" => {
            tracing::error!(
                plugin_id,
                agent_id,
                level,
                message_bytes,
                field_count,
                "plugin_log_emit"
            )
        }
        "warn" => {
            tracing::warn!(
                plugin_id,
                agent_id,
                level,
                message_bytes,
                field_count,
                "plugin_log_emit"
            )
        }
        "debug" => {
            tracing::debug!(
                plugin_id,
                agent_id,
                level,
                message_bytes,
                field_count,
                "plugin_log_emit"
            )
        }
        _ => {
            tracing::info!(
                plugin_id,
                agent_id,
                level,
                message_bytes,
                field_count,
                "plugin_log_emit"
            )
        }
    }
    json!({ "logged": true })
}

#[cfg(test)]
mod tests {
    use super::{LogEmitArgs, log_emit, time_now};
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct TraceWriter(Arc<Mutex<Vec<u8>>>);

    struct TraceWriterGuard(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for TraceWriterGuard {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("trace buffer poisoned").extend(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TraceWriter {
        type Writer = TraceWriterGuard;

        fn make_writer(&'a self) -> Self::Writer {
            TraceWriterGuard(Arc::clone(&self.0))
        }
    }

    #[test]
    fn log_emit_traces_metadata_without_message_or_field_values() {
        let writer = TraceWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        log_emit(
            LogEmitArgs {
                level: Some("warn".into()),
                message: "LOG_MESSAGE_SENTINEL".into(),
                fields: Some(json!({
                    "safe_field": "FIELD_VALUE_SENTINEL",
                    "authorization": "AUTHORIZATION_SENTINEL",
                    "very_long_field_name_that_must_not_be_logged_verbatim": "LONG_SENTINEL"
                })),
            },
            Some(7),
            Some(9),
        );

        let output =
            String::from_utf8(writer.0.lock().expect("trace buffer poisoned").clone()).unwrap();
        assert!(output.contains("plugin_log_emit"));
        assert!(output.contains("message_bytes=20"));
        assert!(output.contains("field_count=3"));
        for sentinel in [
            "LOG_MESSAGE_SENTINEL",
            "FIELD_VALUE_SENTINEL",
            "AUTHORIZATION_SENTINEL",
            "LONG_SENTINEL",
            "authorization",
            "safe_field",
            "very_long_field_name_that_must_not_be_logged_verbatim",
        ] {
            assert!(
                !output.contains(sentinel),
                "trace leaked {sentinel}: {output}"
            );
        }
    }

    #[test]
    fn utility_replies_match_the_host_function_contract() {
        let now = time_now();
        assert!(
            now.get("unix_ms")
                .and_then(|value| value.as_i64())
                .is_some()
        );
        assert!(
            now.get("rfc3339")
                .and_then(|value| value.as_str())
                .is_some()
        );
        assert!(now.get("iso").is_none());

        let logged = log_emit(
            LogEmitArgs {
                level: Some("info".into()),
                message: "not returned".into(),
                fields: None,
            },
            Some(1),
            Some(2),
        );
        assert_eq!(logged, json!({"logged": true}));
    }
}
