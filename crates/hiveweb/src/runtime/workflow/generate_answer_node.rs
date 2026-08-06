//! Workflow DAG 执行器（research §5 / FR-013 / US3 / T110）
//!
//! 拓扑序 BFS 分层 + `tokio::join_all` 并行同层节点。环检测在 service 层
//! `services::workflow::put_graph` 保存时已禁止（spec FR-014），运行时不再校验。
//!
//! mapping 解析：每个 edge 的 `mapping = {"dst.input.<field>": "<src_node_key>.output.<path>"}`
//! 路径 `<path>` 支持简单 dot-path (e.g. `temp_c` 或 `nested.field`)。

use serde_json::Value;
use std::sync::Arc;

use super::{ExecutorDeps, WorkflowError};
use crate::runtime::llm_audit::{
    LlmAuditGuard, LlmAuditSource, LlmLocalFallbackReason, record_local_fallback,
};
use crate::services::runtime_audit;
use agent::context::AgentContext;

/// Replace `{var_name}` template variables in a string with values from input JSON.
pub fn resolve_template_vars(template: &str, input: &Value) -> String {
    let mut result = template.to_string();
    if let Value::Object(map) = input {
        for (key, val) in map {
            let placeholder = format!("{{{key}}}");
            let replacement = match val {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            result = result.replace(&placeholder, &replacement);
        }
    }
    // Replace remaining unresolved placeholders with empty string.
    // Supports optional whitespace: { var } or {var}
    let re = regex::Regex::new(r"\{\s*[a-zA-Z0-9_]+\s*\}")
        .unwrap_or_else(|_| regex::Regex::new(r"\{[^}]+\}").unwrap());
    re.replace_all(&result, "").to_string()
}

/// Build conversation history context from AgentContext messages.
///
/// When `limit > 0`, returns the most recent `limit` messages from the
/// conversation history stored in AgentContext, formatted as role:content pairs.
pub fn build_history_context(agent_ctx: &AgentContext, limit: usize) -> String {
    let messages: Vec<serde_json::Value> = match agent_ctx.get_messages() {
        Ok(msgs) if !msgs.is_empty() => msgs,
        Ok(_) | Err(_) => {
            return format!("用户A:{}", agent_ctx.user_input().raw_text);
        }
    };

    // Take the most recent `limit` messages when limit > 0
    let recent: Vec<&serde_json::Value> = if limit > 0 && messages.len() > limit {
        let skip = messages.len() - limit;
        messages.iter().skip(skip).collect()
    } else {
        messages.iter().collect()
    };

    let mut lines: Vec<String> = Vec::with_capacity(recent.len() + 1);
    for msg in &recent {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
        if !content.is_empty() {
            let user = if role == "user" {
                "用户A"
            } else if role == "assistant" {
                "用户B"
            } else {
                role
            };
            lines.push(format!("{user}: {content}"));
        }
    }

    lines.join("\n")
}

/// Extract the set of keys referenced by `{var}` placeholders in a template.
/// Supports optional whitespace: `{ var }` and `{var}` both match.
#[expect(
    dead_code,
    reason = "retained for the pending answer-node prompt composition path"
)]
fn referenced_template_keys(template: &str) -> std::collections::HashSet<String> {
    let re = regex::Regex::new(r"\{\s*([a-zA-Z0-9_]+)\s*\}")
        .unwrap_or_else(|_| regex::Regex::new(r"\{[^}]+\}").unwrap());
    re.captures_iter(template)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

fn parse_model_preset<'a>(
    config: &'a Value,
    node_key: &str,
) -> Result<Option<&'a str>, WorkflowError> {
    match config.get("model_preset") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(name)) => Ok(Some(name.as_str())),
        Some(_) => Err(WorkflowError::NodeFailure {
            node_key: node_key.to_string(),
            message: "model_preset must be a string or null".to_string(),
        }),
    }
}

fn model_preset_resolution_error(model_preset: Option<&str>, node_key: &str) -> WorkflowError {
    model_preset.map_or_else(
        || WorkflowError::NodeFailure {
            node_key: node_key.to_string(),
            message: "model_preset is unavailable".to_string(),
        },
        |name| WorkflowError::ModelPresetUnknown(name.to_string()),
    )
}

/// Execute a generate-answer node: resolves template variables from upstream outputs
/// and calls LLM to generate a response.
pub async fn execute_answer_node(
    deps: &ExecutorDeps,
    node_key: &str,
    mut input: Value,
    node_config: Option<Value>,
    invoking_agent_id: i64,
    agent_ctx: Arc<AgentContext>,
) -> Result<(String, Value), WorkflowError> {
    let config = node_config.unwrap_or_default();
    let system_prompt = config
        .get("system_prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("You are a helpful assistant.");
    let model_preset = parse_model_preset(&config, node_key)?;
    let history_window = config
        .get("history_window")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0) as usize;

    // Strip _agent_context from input — it's runtime machinery, not user data
    if let Value::Object(ref mut map) = input {
        map.remove("_agent_context");
    }

    // When history_window > 0, build conversation history from AgentContext
    // messages and inject it as {context} for system_prompt template replacement.
    if history_window > 0 {
        let history_context = build_history_context(&agent_ctx, history_window);
        if !history_context.is_empty()
            && let Value::Object(ref mut map) = input
        {
            map.insert("context".to_string(), Value::String(history_context));
        }
    }

    // Resolve template variables in system_prompt (e.g., "{query}" → actual value,
    // "{context}" → conversation history).
    let resolved_prompt = resolve_template_vars(system_prompt, &input);

    tracing::info!(
        node_key_fingerprint = %runtime_audit::identifier_fingerprint(node_key),
        system_prompt_len = system_prompt.len(),
        resolved_prompt_len = resolved_prompt.len(),
        input_field_count = input.as_object().map_or(0, serde_json::Map::len),
        query_bytes = input
            .get("query")
            .and_then(|value| value.as_str())
            .map_or(0, str::len),
        "execute_answer_node: invoking LLM"
    );

    let resolved_preset_name = model_preset.or(deps.llm.default_name.as_deref());
    let mut llm_audit = LlmAuditGuard::new(
        deps.execution_context.clone(),
        Some(invoking_agent_id),
        resolved_preset_name,
        LlmAuditSource::GenerateAnswer,
    );
    let (provider, model) = match deps.llm.build_chain(model_preset) {
        Ok(chain) => chain,
        Err(_) => {
            llm_audit.finish_model_preset_unknown();
            return Err(model_preset_resolution_error(model_preset, node_key));
        }
    };
    // Resolve defaults only after the explicit preset has passed fail-closed
    // provider-chain resolution, so an unknown explicit name is never
    // substituted with another preset's generation settings.
    let (max_tokens, _temperature) = match deps.llm.resolve_generation_defaults(model_preset) {
        Ok(defaults) => defaults,
        Err(_) => {
            llm_audit.finish_model_preset_unknown();
            return Err(model_preset_resolution_error(model_preset, node_key));
        }
    };
    let answer = {
        use providers::{ChatRequest, LlmCallOptions};
        let req = ChatRequest {
            messages: vec![serde_json::json!({"role": "system", "content": resolved_prompt})],
            model: Some(model),
            max_tokens,
            temperature: 0f32,
            tools: None,
            tool_choice: None,
            reasoning_effort: None,
        };
        let options = LlmCallOptions::default().with_fallback_callback(llm_audit.on_fallback());
        let resp = provider.chat_with_options(req, options).await;
        llm_audit.finish_response(&resp);
        if resp.is_error() {
            record_local_fallback(
                &deps.execution_context,
                Some(invoking_agent_id),
                resolved_preset_name,
                LlmAuditSource::GenerateAnswer,
                LlmLocalFallbackReason::InvocationFailed,
            );
            let err_msg = resp
                .content
                .unwrap_or_else(|| "unknown LLM error".to_string());
            tracing::warn!(
                node_key_fingerprint = %runtime_audit::identifier_fingerprint(node_key),
                error_kind = "llm_invocation_failed",
                "answer node LLM failed, using fallback"
            );
            format!("[LLM 调用失败: {err_msg}]")
        } else {
            resp.content.unwrap_or_default()
        }
    };

    Ok((
        node_key.to_string(),
        serde_json::json!({
            "answer": answer,
            "model_preset": model_preset,
        }),
    ))
}

/// Build a human-readable user message from the resolved node input.
///
/// Priority: well-known question key (query/text/etc) → first string field → "key: value" dump.
/// Keys referenced by `{var}` in system_prompt are excluded from the dump to avoid duplication.
#[expect(
    dead_code,
    reason = "retained for the pending answer-node prompt composition path"
)]
fn build_user_message(input: &Value, system_prompt: &str) -> String {
    const USER_QUESTION_KEYS: &[&str] = &[
        "query",
        "question",
        "text",
        "input",
        "message",
        "prompt",
        "user_input",
        "raw_text",
        "user_message",
        "ask",
    ];

    if let Value::Object(map) = input {
        for key in USER_QUESTION_KEYS {
            if let Some(Value::String(s)) = map.get(*key) {
                return s.clone();
            }
        }
        for (k, v) in map {
            if !k.starts_with('_')
                && let Value::String(s) = v
            {
                return s.clone();
            }
        }
        let re = regex::Regex::new(r"\{\s*([a-zA-Z0-9_]+)\s*\}")
            .unwrap_or_else(|_| regex::Regex::new(r"\{[^}]+\}").unwrap());
        let referenced: std::collections::HashSet<String> = re
            .captures_iter(system_prompt)
            .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
            .collect();
        let lines: Vec<String> = map
            .iter()
            .filter(|(k, _)| !referenced.contains(*k) && !k.starts_with('_'))
            .map(|(k, v)| {
                let val = serde_json::to_string(v).unwrap_or_default();
                format!("{k}: {val}")
            })
            .collect();
        if !lines.is_empty() {
            return lines.join("\n");
        }
    }
    serde_json::to_string(input).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::capability::CapabilityRegistry;
    use crate::runtime::execution_context::RuntimeExecutionContext;
    use crate::runtime::invoker::Invoker;
    use crate::runtime::llm::LlmRegistry;
    use crate::runtime::pool::{InstancePool, PoolConfig};
    use agent::context::{ContextConfig, UserInput};
    use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions};
    use std::collections::HashMap;
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[derive(Clone, Default)]
    struct TraceWriter(Arc<Mutex<Vec<u8>>>);

    struct TraceWriterGuard(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for TraceWriterGuard {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.lock().expect("trace buffer poisoned").extend(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TraceWriter {
        type Writer = TraceWriterGuard;

        fn make_writer(&'a self) -> Self::Writer {
            TraceWriterGuard(Arc::clone(&self.0))
        }
    }

    fn agent_context() -> Arc<AgentContext> {
        Arc::new(AgentContext::new(
            "generate-answer-test".to_string(),
            UserInput {
                raw_text: "hello".to_string(),
                session_id: None,
                message_id: None,
                timestamp: chrono::Utc::now(),
                metadata: HashMap::new(),
            },
            ContextConfig::default(),
        ))
    }

    fn executor_deps(llm: Arc<LlmRegistry>) -> ExecutorDeps {
        let pool = MySqlPoolOptions::new().connect_lazy_with(MySqlConnectOptions::new());
        let instance_pool = InstancePool::new(PoolConfig {
            max_per_plugin: 1,
            max_total: 1,
            idle_timeout: Duration::from_secs(60),
            acquire_timeout: Duration::from_millis(50),
            call_timeout_ms: 5_000,
            call_memory_mb: 128,
            call_fuel: 10_000_000,
        });
        ExecutorDeps {
            execution_context: RuntimeExecutionContext::best_effort(None, None).for_hook(),
            pool,
            s3: None,
            registry: Arc::new(CapabilityRegistry::new()),
            llm,
            invoker: Arc::new(Invoker::new(instance_pool)),
            ext_pool: None,
            redis: None,
            permissions: Vec::new(),
        }
    }

    async fn read_http_request(stream: &mut tokio::net::TcpStream) {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let header_end = loop {
            let read = stream.read(&mut buffer).await.expect("read LLM request");
            assert_ne!(read, 0, "LLM request ended before its headers");
            request.extend_from_slice(&buffer[..read]);
            if let Some(position) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break position + 4;
            }
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length").then(|| {
                    value
                        .trim()
                        .parse::<usize>()
                        .expect("numeric content length")
                })
            })
            .unwrap_or(0);
        while request.len() < header_end + content_length {
            let read = stream.read(&mut buffer).await.expect("read LLM body");
            assert_ne!(read, 0, "LLM request ended before its body");
            request.extend_from_slice(&buffer[..read]);
        }
    }

    async fn registry_with_default_server(
        status: &'static str,
        response_body: &'static str,
    ) -> (
        Arc<LlmRegistry>,
        Arc<AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind default LLM provider");
        let address = listener.local_addr().expect("default LLM address");
        let request_count = Arc::new(AtomicUsize::new(0));
        let server_count = Arc::clone(&request_count);
        let server = tokio::spawn(async move {
            let accepted =
                tokio::time::timeout(Duration::from_millis(250), listener.accept()).await;
            let Ok(Ok((mut stream, _))) = accepted else {
                return;
            };
            server_count.fetch_add(1, Ordering::SeqCst);
            read_http_request(&mut stream).await;
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write LLM response");
        });

        let path = std::env::temp_dir().join(format!(
            "hiveweb-generate-answer-presets-{}.toml",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &path,
            format!(
                r#"
[[preset]]
name = "default"
description = "default"
default = true
max_tokens = 321
temperature = 0.4

  [[preset.providers]]
  kind = "openai_compat"
  model = "default-model"
  auth = "none"
  base_url = "http://{address}/v1"
"#
            ),
        )
        .expect("write LLM preset");
        let registry =
            LlmRegistry::load_from_path(path.to_str().expect("UTF-8 temp path")).expect("load LLM");
        std::fs::remove_file(path).expect("remove LLM preset");

        (registry, request_count, server)
    }

    fn assert_unknown_preset(result: Result<(String, Value), WorkflowError>, expected: &str) {
        assert!(matches!(
            result,
            Err(WorkflowError::ModelPresetUnknown(name)) if name == expected
        ));
    }

    fn assert_preset_type_failure(
        result: Result<(String, Value), WorkflowError>,
        expected_node_key: &str,
    ) {
        assert!(matches!(
            result,
            Err(WorkflowError::NodeFailure { node_key, message })
                if node_key == expected_node_key && message.contains("model_preset")
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn explicit_empty_model_preset_fails_closed_without_requesting_default() {
        let (llm, request_count, server) = registry_with_default_server(
            "200 OK",
            r#"{"choices":[{"message":{"content":"default called"},"finish_reason":"stop"}],"usage":{}}"#,
        )
        .await;
        let deps = executor_deps(llm);

        let result = execute_answer_node(
            &deps,
            "empty-preset",
            serde_json::json!({"query": "hello"}),
            Some(serde_json::json!({"model_preset": ""})),
            7,
            agent_context(),
        )
        .await;
        server.await.expect("default provider server task");

        assert_unknown_preset(result, "");
        assert_eq!(
            request_count.load(Ordering::SeqCst),
            0,
            "an explicit empty preset must never resolve to the default provider"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn explicit_unknown_model_preset_fails_closed_without_fake_fallback_audit() {
        let writer = TraceWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer.clone())
            .finish();
        let _subscriber = tracing::subscriber::set_default(subscriber);
        let (llm, request_count, server) = registry_with_default_server(
            "200 OK",
            r#"{"choices":[{"message":{"content":"default called"},"finish_reason":"stop"}],"usage":{}}"#,
        )
        .await;
        let deps = executor_deps(llm);

        let result = execute_answer_node(
            &deps,
            "unknown-preset",
            serde_json::json!({"query": "hello"}),
            Some(serde_json::json!({"model_preset": "missing"})),
            7,
            agent_context(),
        )
        .await;
        server.await.expect("default provider server task");

        assert_unknown_preset(result, "missing");
        assert_eq!(request_count.load(Ordering::SeqCst), 0);
        let traces =
            String::from_utf8(writer.0.lock().expect("trace buffer poisoned").clone()).unwrap();
        assert!(
            traces.contains("event_type=\"llm_invoke\"") && traces.contains("model_preset_unknown"),
            "configuration errors must produce one typed LLM invocation audit: {traces}"
        );
        assert!(
            !traces.contains("event_type=\"llm_fallback\""),
            "configuration errors must not be recorded as successful LLM fallback"
        );
        assert!(
            !traces.contains("event_type=\"llm_local_fallback\""),
            "configuration errors must fail closed without a local text fallback"
        );
        assert!(
            !traces.contains("[无可用模型]"),
            "configuration errors must not fabricate a local answer"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn json_null_model_preset_uses_the_default_provider() {
        let (llm, request_count, server) = registry_with_default_server(
            "200 OK",
            r#"{"choices":[{"message":{"content":"default answer"},"finish_reason":"stop"}],"usage":{}}"#,
        )
        .await;
        let deps = executor_deps(llm);

        let result = execute_answer_node(
            &deps,
            "null-preset",
            serde_json::json!({"query": "hello"}),
            Some(serde_json::json!({"model_preset": null})),
            7,
            agent_context(),
        )
        .await
        .expect("JSON null must use the default preset");
        server.await.expect("default provider server task");

        assert_eq!(request_count.load(Ordering::SeqCst), 1);
        assert_eq!(result.1["answer"], "default answer");
        assert_eq!(result.1["model_preset"], Value::Null);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn missing_model_preset_uses_the_default_provider() {
        let (llm, request_count, server) = registry_with_default_server(
            "200 OK",
            r#"{"choices":[{"message":{"content":"default answer"},"finish_reason":"stop"}],"usage":{}}"#,
        )
        .await;
        let deps = executor_deps(llm);

        let result = execute_answer_node(
            &deps,
            "missing-preset",
            serde_json::json!({"query": "hello"}),
            Some(serde_json::json!({})),
            7,
            agent_context(),
        )
        .await
        .expect("a missing field must use the default preset");
        server.await.expect("default provider server task");

        assert_eq!(request_count.load(Ordering::SeqCst), 1);
        assert_eq!(result.1["answer"], "default answer");
        assert_eq!(result.1["model_preset"], Value::Null);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn non_string_model_preset_fails_closed_without_requesting_default() {
        let (llm, request_count, server) = registry_with_default_server(
            "200 OK",
            r#"{"choices":[{"message":{"content":"default called"},"finish_reason":"stop"}],"usage":{}}"#,
        )
        .await;
        let deps = executor_deps(llm);

        let result = execute_answer_node(
            &deps,
            "typed-preset",
            serde_json::json!({"query": "hello"}),
            Some(serde_json::json!({"model_preset": 7})),
            7,
            agent_context(),
        )
        .await;
        server.await.expect("default provider server task");

        assert_preset_type_failure(result, "typed-preset");
        assert_eq!(request_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn provider_error_keeps_the_existing_local_failure_text() {
        let writer = TraceWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer.clone())
            .finish();
        let _subscriber = tracing::subscriber::set_default(subscriber);
        let (llm, request_count, server) = registry_with_default_server(
            "503 Service Unavailable",
            r#"{"error":{"type":"server_error","message":"temporarily unavailable"}}"#,
        )
        .await;
        let deps = executor_deps(llm);

        let result = execute_answer_node(
            &deps,
            "provider-error",
            serde_json::json!({"query": "hello"}),
            Some(serde_json::json!({"model_preset": null})),
            7,
            agent_context(),
        )
        .await
        .expect("provider errors retain the local text fallback");
        server.await.expect("default provider server task");

        assert_eq!(request_count.load(Ordering::SeqCst), 1);
        let answer = result.1["answer"].as_str().expect("answer text");
        assert!(answer.starts_with("[LLM 调用失败:"));
        assert!(!answer.starts_with("[无可用模型]"));
        let traces =
            String::from_utf8(writer.0.lock().expect("trace buffer poisoned").clone()).unwrap();
        assert_eq!(
            traces.matches("event_type=\"llm_invoke\"").count(),
            1,
            "the provider call must have one terminal invocation audit: {traces}"
        );
        assert_eq!(
            traces.matches("event_type=\"llm_local_fallback\"").count(),
            1,
            "the application-generated text must have one local fallback audit: {traces}"
        );
        assert!(
            !traces.contains("event_type=\"llm_fallback\""),
            "a local text fallback is not a provider transition: {traces}"
        );
    }
}
