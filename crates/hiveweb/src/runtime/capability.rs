//! Capability 静态注册表 + Dispatcher (FR-001 / FR-003 / data-model §6)
//!
//! Capability 是宿主侧零信任默认 deny 的能力点；Agent 的 permissions 必须显式
//! 列出某 capability 才允许 Plugin 在调用时使用它。注册表 = 代码侧真值源；DB
//! `capabilities` 表 = 启动期 upsert 的镜像（V008 + V018 seed）。
//!
//! Dispatcher（T102）：
//!   Plugin host_call(envelope) -> 解析 -> 鉴权 (Agent.permissions) ->
//!   未知 capability 返 4045 -> handler -> audit -> 返回 envelope。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::MySqlPool;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use hive_runtime_core::abi::StableErrorKind;

use crate::runtime::capabilities;
use crate::runtime::capabilities::{CapabilityFailure, CapabilityFailureKind};
use crate::runtime::execution_context::RuntimeExecutionContext;
use crate::services::runtime_audit::{self, AuditRecord};
use crate::utils::error::codes;

pub const HOST_CALL_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Capability 名常量（与 data-model §V018 / §6 + 扩展对齐）
pub const NETWORK_HTTP: &str = "network.http";
pub const FS_READ: &str = "fs.read";
pub const FS_WRITE: &str = "fs.write";
pub const S3_READ: &str = "s3.read";
pub const S3_WRITE: &str = "s3.write";
pub const DB_QUERY: &str = "db.query";
pub const DB_EXECUTE: &str = "db.execute";
pub const LLM_INVOKE: &str = "llm.invoke";
pub const SECRET_GET: &str = "secret.get";
pub const TIME_NOW: &str = "time.now";
pub const LOG_EMIT: &str = "log.emit";
/// 扩展：内置函数/工具所需的细粒度 capability
pub const CHAT_RESPOND: &str = "chat_respond";
pub const EXEC_RUN: &str = "exec.run";
pub const AGENT_SPAWN: &str = "agent.spawn";
pub const CRON_MANAGE: &str = "cron.manage";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    pub name: &'static str,
    pub description: &'static str,
    pub is_dangerous: bool,
}

pub const CAPABILITIES: &[Capability] = &[
    Capability {
        name: NETWORK_HTTP,
        description: "HTTP/HTTPS access (allowlisted hosts; SSRF-blocked)",
        is_dangerous: true,
    },
    Capability {
        name: FS_READ,
        description: "/tmp/plugin/ 内文件读",
        is_dangerous: false,
    },
    Capability {
        name: FS_WRITE,
        description: "/tmp/plugin/ 内文件写",
        is_dangerous: false,
    },
    Capability {
        name: S3_READ,
        description: "Rustfs 桶 GET",
        is_dangerous: false,
    },
    Capability {
        name: S3_WRITE,
        description: "Rustfs 桶 PUT / DELETE",
        is_dangerous: false,
    },
    Capability {
        name: DB_QUERY,
        description: "宿主预注册命名 SELECT 查询",
        is_dangerous: false,
    },
    Capability {
        name: DB_EXECUTE,
        description: "宿主预注册命名 DML（永不自由 SQL）",
        is_dangerous: true,
    },
    Capability {
        name: LLM_INVOKE,
        description: "LLM 调用（走 Agent.model_preset 解析）",
        is_dangerous: false,
    },
    Capability {
        name: SECRET_GET,
        description: "allowlist 内的密钥读取",
        is_dangerous: true,
    },
    Capability {
        name: TIME_NOW,
        description: "服务器当前时间",
        is_dangerous: false,
    },
    Capability {
        name: LOG_EMIT,
        description: "结构化日志写入（rate-limited）",
        is_dangerous: false,
    },
    Capability {
        name: CHAT_RESPOND,
        description: "提交 Agent 最终用户可见回复",
        is_dangerous: false,
    },
    Capability {
        name: EXEC_RUN,
        description: "Shell 命令执行（受 workspace 边界约束）",
        is_dangerous: true,
    },
    Capability {
        name: AGENT_SPAWN,
        description: "生成子 Agent 执行独立任务",
        is_dangerous: false,
    },
    Capability {
        name: CRON_MANAGE,
        description: "管理定时 Cron 任务",
        is_dangerous: false,
    },
];

#[derive(Debug, Clone)]
pub struct CapabilityRegistry {
    index: HashMap<&'static str, &'static Capability>,
}

impl CapabilityRegistry {
    pub fn new() -> Self {
        let mut index = HashMap::with_capacity(CAPABILITIES.len());
        for cap in CAPABILITIES {
            index.insert(cap.name, cap);
        }
        Self { index }
    }

    pub fn lookup(&self, name: &str) -> Option<&'static Capability> {
        self.index.get(name).copied()
    }

    pub fn is_dangerous(&self, name: &str) -> bool {
        self.lookup(name).map(|c| c.is_dangerous).unwrap_or(false)
    }

    pub fn all(&self) -> &'static [Capability] {
        CAPABILITIES
    }
}

impl Default for CapabilityRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ============================== Dispatcher ==============================

/// host_call 调用上下文，由 invoker 在每次 Plugin 调用前注入
#[derive(Debug, Clone)]
pub struct DispatchCtx {
    pub execution_context: RuntimeExecutionContext,
    pub agent_id: i64,
    pub plugin_id: i64,
    pub function_id: Option<i64>,
    /// 预计算的 agent 权限列表（测试模式可为全量）；若为空则 fallback 到 DB 查询
    pub permissions: Vec<String>,
}

/// Plugin 侧发来的 envelope: { "capability": "...", "args": {...} }
#[derive(Debug, Deserialize)]
pub struct CallEnvelope {
    pub capability: String,
    #[serde(default)]
    pub args: Value,
}

/// 宿主返回给 Plugin 的 envelope: ok / error 互斥
#[derive(Debug, Serialize)]
pub struct ReplyEnvelope {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// 将共享 ABI 的稳定错误类别映射为 host_call 数值码（u16）。
///
/// 共享 [`StableErrorKind::host_call_code`] 返回 `Option<u32>`；hiveweb 的
/// `ReplyEnvelope.code` 为 `u16`。所有会走到 host_call 返回路径的共享码均
/// `< 65536`，转换无损。`FunctionNotExecutable` 无 host_call 码，调用点不会
/// 传入，故 `expect` 是静态不变量。
fn stable_code(kind: StableErrorKind) -> u16 {
    kind.host_call_code()
        .expect("this StableErrorKind variant has a host_call code") as u16
}

impl ReplyEnvelope {
    pub fn ok(data: Value) -> Self {
        Self {
            ok: true,
            data: Some(data),
            code: None,
            message: None,
        }
    }
    pub fn err(code: u16, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            code: Some(code),
            message: Some(message.into()),
        }
    }
}

fn serialize_bounded_reply(reply: ReplyEnvelope) -> (String, bool) {
    let encoded = serde_json::to_string(&reply).unwrap_or_else(|_| {
        r#"{"ok":false,"code":5000,"message":"Capability response serialization failed"}"#
            .to_string()
    });
    if encoded.len() <= HOST_CALL_MAX_BYTES {
        return (encoded, false);
    }

    (
        serde_json::to_string(&ReplyEnvelope::err(
            stable_code(StableErrorKind::Internal),
            "Capability response exceeds 4 MiB limit",
        ))
        .expect("static bounded capability error must serialize"),
        true,
    )
}

/// 从 DB 查 agent 的 capability 集合（带简易 in-memory 缓存可在 invoker 层加）。
async fn load_agent_permissions(
    pool: &MySqlPool,
    agent_id: i64,
) -> Result<HashSet<String>, sqlx::Error> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT capability FROM agent_permissions WHERE agent_id = ?")
            .bind(agent_id)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(|(c,)| c).collect())
}

/// Dispatcher 调用所需的宿主资源句柄（pool / s3 / registry / llm）
#[derive(Clone)]
pub struct DispatcherDeps {
    pub pool: MySqlPool,
    /// 仅在 `PLUGIN_SYSTEM_ENABLED=true` 时为 `Some`；`s3.*` capability 必须检查。
    pub s3: Option<aws_sdk_s3::Client>,
    pub registry: Arc<CapabilityRegistry>,
    pub llm: Arc<crate::runtime::llm::LlmRegistry>,
    /// Process-wide in production; injectable in tests to isolate limiter state.
    pub rate_limits: Arc<capabilities::rate_limit::CapabilityRateLimits>,
}

impl std::fmt::Debug for DispatcherDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatcherDeps").finish_non_exhaustive()
    }
}

fn to_reply<T: Serialize>(
    capability: &str,
    result: Result<T, CapabilityFailure>,
) -> (
    ReplyEnvelope,
    &'static str,
    Option<runtime_audit::SafeAuditError>,
) {
    match result {
        Ok(value) => (
            ReplyEnvelope::ok(serde_json::to_value(value).unwrap_or(Value::Null)),
            "success",
            None,
        ),
        Err(error) => {
            let safe_error =
                runtime_audit::safe_capability_error_for_kind(capability, error.audit_kind());
            let code = match error.kind() {
                CapabilityFailureKind::InvalidArguments => {
                    stable_code(StableErrorKind::InvalidArgs)
                }
                CapabilityFailureKind::Timeout => stable_code(StableErrorKind::CapabilityTimeout),
                // HiveWeb 产品扩展：模型预设未知不在共享 `StableErrorKind`
                // 表内（`llm.invoke` 能力级错误，对应 API 层 5007 / HTTP 422）。
                CapabilityFailureKind::ModelPresetUnknown => codes::MODEL_PRESET_UNKNOWN,
                CapabilityFailureKind::Failed => stable_code(StableErrorKind::Internal),
            };
            (
                ReplyEnvelope::err(code, safe_error.message),
                "error",
                Some(safe_error),
            )
        }
    }
}

fn static_handler_result<T, E>(result: Result<T, E>) -> Result<T, CapabilityFailure> {
    result.map_err(|_| CapabilityFailure::failed("capability handler failed"))
}

/// HiveWeb 产品扩展错误：能力限流（4292）不在共享 `StableErrorKind` 表内。
/// 它是宿主侧的服务拒绝（HTTP 429 语义），非 Plugin ABI 的通用错误类别；
/// HiveGUI 本地执行无限流，故不共享。
fn rate_limited_reply() -> (
    ReplyEnvelope,
    &'static str,
    Option<runtime_audit::SafeAuditError>,
) {
    (
        ReplyEnvelope::err(
            codes::CAPABILITY_RATE_LIMITED,
            "Capability rate limit exceeded",
        ),
        "denied",
        Some(runtime_audit::SafeAuditError {
            kind: "capability_rate_limited",
            message: "Capability rate limit exceeded",
        }),
    )
}

/// Dispatcher 主入口。返回的 JSON 字符串会被 Plugin 侧解码成 ReplyEnvelope。
pub async fn dispatch(deps: &DispatcherDeps, ctx: &DispatchCtx, envelope_str: &str) -> String {
    let pool = &deps.pool;
    let registry = &deps.registry;
    let t0 = Instant::now();

    if envelope_str.len() > HOST_CALL_MAX_BYTES {
        let reply = ReplyEnvelope::err(
            stable_code(StableErrorKind::InvalidArgs),
            "Payload exceeds 4 MiB limit",
        );
        runtime_audit::record(
            &ctx.execution_context,
            AuditRecord {
                agent_id: Some(ctx.agent_id),
                plugin_id: Some(ctx.plugin_id),
                function_id: ctx.function_id,
                capability: None,
                event_type: "capability_call",
                outcome: "error",
                elapsed_ms: Some(t0.elapsed().as_millis() as i32),
                error_message: Some("Capability payload exceeded the host_call limit"),
                payload_summary: None,
            },
        );
        return serialize_bounded_reply(reply).0;
    }

    let parsed: Result<CallEnvelope, _> = serde_json::from_str(envelope_str);
    let envelope = match parsed {
        Ok(e) => e,
        Err(_) => {
            // Envelope 解析失败统一映射为共享 ABI 的 `invalid_args` (4001)，
            // 而非 API 层通用码 `BAD_REQUEST` (4000)：host_call 返回的是
            // Plugin ABI 错误码，非 HTTP 业务码（contract §3）。
            let reply = ReplyEnvelope::err(
                stable_code(StableErrorKind::InvalidArgs),
                "Invalid host_call envelope",
            );
            runtime_audit::record(
                &ctx.execution_context,
                AuditRecord {
                    agent_id: Some(ctx.agent_id),
                    plugin_id: Some(ctx.plugin_id),
                    function_id: ctx.function_id,
                    capability: None,
                    event_type: "capability_call",
                    outcome: "error",
                    elapsed_ms: Some(t0.elapsed().as_millis() as i32),
                    error_message: Some("invalid envelope"),
                    payload_summary: None,
                },
            );
            return serialize_bounded_reply(reply).0;
        }
    };

    // 1. unknown capability → 4045
    let cap_name = envelope.capability.clone();
    if registry.lookup(&cap_name).is_none() {
        let reply = ReplyEnvelope::err(
            stable_code(StableErrorKind::CapabilityUnknown),
            "Unknown capability",
        );
        runtime_audit::record(
            &ctx.execution_context,
            AuditRecord {
                agent_id: Some(ctx.agent_id),
                plugin_id: Some(ctx.plugin_id),
                function_id: ctx.function_id,
                capability: None,
                event_type: "capability_denied",
                outcome: "denied",
                elapsed_ms: Some(t0.elapsed().as_millis() as i32),
                error_message: Some("unknown capability"),
                payload_summary: None,
            },
        );
        return serialize_bounded_reply(reply).0;
    }

    // 2. permission check
    let granted: HashSet<String> = if !ctx.permissions.is_empty() {
        ctx.permissions.iter().cloned().collect()
    } else {
        match load_agent_permissions(pool, ctx.agent_id).await {
            Ok(s) => s,
            Err(_) => {
                tracing::error!(
                    error_kind = "permission_lookup_failed",
                    agent_id = ctx.agent_id,
                    "load_agent_permissions"
                );
                let reply = ReplyEnvelope::err(
                    stable_code(StableErrorKind::Internal),
                    "permission lookup failed",
                );
                runtime_audit::record(
                    &ctx.execution_context,
                    AuditRecord {
                        agent_id: Some(ctx.agent_id),
                        plugin_id: Some(ctx.plugin_id),
                        function_id: ctx.function_id,
                        capability: Some(&cap_name),
                        event_type: "capability_call",
                        outcome: "error",
                        elapsed_ms: Some(t0.elapsed().as_millis() as i32),
                        error_message: Some("permission lookup failed"),
                        payload_summary: None,
                    },
                );
                return serialize_bounded_reply(reply).0;
            }
        }
    };
    if !granted.contains(&cap_name) {
        let reply = ReplyEnvelope::err(
            stable_code(StableErrorKind::CapabilityDenied),
            format!("当前 Agent 未授权调用能力「{cap_name}」"),
        );
        runtime_audit::record(
            &ctx.execution_context,
            AuditRecord {
                agent_id: Some(ctx.agent_id),
                plugin_id: Some(ctx.plugin_id),
                function_id: ctx.function_id,
                capability: Some(&cap_name),
                event_type: "capability_denied",
                outcome: "denied",
                elapsed_ms: Some(t0.elapsed().as_millis() as i32),
                error_message: Some("capability not granted"),
                payload_summary: runtime_audit::summarize_capability_call(
                    &cap_name,
                    &envelope.args,
                    "denied",
                    Some("capability_not_granted"),
                ),
            },
        );
        return serialize_bounded_reply(reply).0;
    }

    // 3. handler dispatch
    fn args_err(
        _label: &str,
        _error: serde_json::Error,
    ) -> (
        ReplyEnvelope,
        &'static str,
        Option<runtime_audit::SafeAuditError>,
    ) {
        (
            ReplyEnvelope::err(
                stable_code(StableErrorKind::InvalidArgs),
                "Invalid capability arguments",
            ),
            "error",
            Some(runtime_audit::SafeAuditError {
                kind: "invalid_capability_args",
                message: "Capability arguments were invalid",
            }),
        )
    }

    let (reply, mut outcome, mut audit_error) = match cap_name.as_str() {
        TIME_NOW => (
            ReplyEnvelope::ok(capabilities::utility::time_now()),
            "success",
            None,
        ),
        LOG_EMIT => match serde_json::from_value(envelope.args.clone()) {
            Ok(args) => {
                if deps.rate_limits.try_acquire_log(ctx.plugin_id) {
                    let data = capabilities::utility::log_emit(
                        args,
                        Some(ctx.plugin_id),
                        Some(ctx.agent_id),
                    );
                    (ReplyEnvelope::ok(data), "success", None)
                } else {
                    rate_limited_reply()
                }
            }
            Err(e) => args_err("log.emit", e),
        },
        FS_READ => match serde_json::from_value(envelope.args.clone()) {
            Ok(args) => to_reply(
                FS_READ,
                static_handler_result(capabilities::fs::fs_read(args).await),
            ),
            Err(e) => args_err("fs.read", e),
        },
        FS_WRITE => match serde_json::from_value(envelope.args.clone()) {
            Ok(args) => to_reply(
                FS_WRITE,
                static_handler_result(capabilities::fs::fs_write(args).await),
            ),
            Err(e) => args_err("fs.write", e),
        },
        NETWORK_HTTP => match serde_json::from_value(envelope.args.clone()) {
            Ok(args) => match deps
                .rate_limits
                .try_acquire_network(ctx.plugin_id, ctx.execution_context.session_id())
            {
                Some(_permit) => to_reply(
                    NETWORK_HTTP,
                    capabilities::network_http::http_request(args).await,
                ),
                None => rate_limited_reply(),
            },
            Err(e) => args_err("network.http", e),
        },
        S3_READ => match deps.s3.as_ref() {
            // HiveWeb 产品扩展：插件系统关闭（5031）不在共享 `StableErrorKind`
            // 表内，是宿主配置级错误（HTTP 503 语义），非 Plugin ABI 通用类别。
            None => (
                ReplyEnvelope::err(
                    codes::PLUGIN_SYSTEM_DISABLED,
                    "插件系统已关闭，s3.read 不可用",
                ),
                "error",
                Some(runtime_audit::SafeAuditError {
                    kind: "plugin_system_disabled",
                    message: "Plugin system is disabled",
                }),
            ),
            Some(client) => match serde_json::from_value(envelope.args.clone()) {
                Ok(args) => to_reply(
                    S3_READ,
                    static_handler_result(capabilities::s3::s3_read(client, args).await),
                ),
                Err(e) => args_err("s3.read", e),
            },
        },
        S3_WRITE => match deps.s3.as_ref() {
            None => (
                ReplyEnvelope::err(
                    codes::PLUGIN_SYSTEM_DISABLED,
                    "插件系统已关闭，s3.write 不可用",
                ),
                "error",
                Some(runtime_audit::SafeAuditError {
                    kind: "plugin_system_disabled",
                    message: "Plugin system is disabled",
                }),
            ),
            Some(client) => match serde_json::from_value(envelope.args.clone()) {
                Ok(args) => to_reply(
                    S3_WRITE,
                    static_handler_result(capabilities::s3::s3_write(client, args).await),
                ),
                Err(e) => args_err("s3.write", e),
            },
        },
        SECRET_GET => match serde_json::from_value(envelope.args.clone()) {
            Ok(args) => to_reply(
                SECRET_GET,
                static_handler_result(capabilities::secret::secret_get(args)),
            ),
            Err(e) => args_err("secret.get", e),
        },
        DB_QUERY => match serde_json::from_value(envelope.args.clone()) {
            Ok(args) => to_reply(
                DB_QUERY,
                static_handler_result(capabilities::db::db_query(&deps.pool, args).await),
            ),
            Err(e) => args_err("db.query", e),
        },
        DB_EXECUTE => match serde_json::from_value(envelope.args.clone()) {
            Ok(args) => to_reply(
                DB_EXECUTE,
                static_handler_result(capabilities::db::db_execute(&deps.pool, args).await),
            ),
            Err(e) => args_err("db.execute", e),
        },
        LLM_INVOKE => match serde_json::from_value(envelope.args.clone()) {
            Ok(args) => to_reply(
                LLM_INVOKE,
                capabilities::llm::llm_invoke(
                    &deps.pool,
                    &deps.llm,
                    &ctx.execution_context,
                    ctx.agent_id,
                    args,
                )
                .await,
            ),
            Err(e) => args_err("llm.invoke", e),
        },
        _ => (
            ReplyEnvelope::err(
                stable_code(StableErrorKind::Internal),
                "Capability handler is not implemented",
            ),
            "error",
            Some(runtime_audit::SafeAuditError {
                kind: "capability_handler_unimplemented",
                message: "Capability handler is not implemented",
            }),
        ),
    };

    let (encoded_reply, response_exceeded) = serialize_bounded_reply(reply);
    if response_exceeded {
        outcome = "error";
        audit_error = Some(runtime_audit::SafeAuditError {
            kind: "capability_response_too_large",
            message: "Capability response exceeded the host_call limit",
        });
    }
    let error_kind = audit_error.map(|error| error.kind);
    let audit_event_type = if error_kind == Some("capability_rate_limited") {
        "capability_denied"
    } else {
        "capability_call"
    };
    runtime_audit::record(
        &ctx.execution_context,
        AuditRecord {
            agent_id: Some(ctx.agent_id),
            plugin_id: Some(ctx.plugin_id),
            function_id: ctx.function_id,
            capability: Some(&cap_name),
            event_type: audit_event_type,
            outcome,
            elapsed_ms: Some(t0.elapsed().as_millis() as i32),
            error_message: audit_error.map(|error| error.message),
            payload_summary: runtime_audit::summarize_capability_call(
                &cap_name,
                &envelope.args,
                outcome,
                error_kind,
            ),
        },
    );

    encoded_reply
}

#[cfg(test)]
mod tests {
    use super::{
        CapabilityRegistry, DispatchCtx, DispatcherDeps, FS_READ, HOST_CALL_MAX_BYTES, LLM_INVOKE,
        LOG_EMIT, NETWORK_HTTP, ReplyEnvelope, dispatch, serialize_bounded_reply, to_reply,
    };
    use crate::runtime::capabilities::CapabilityFailure;
    use crate::runtime::capabilities::rate_limit::{
        LOG_EMIT_MAX_PER_SECOND, NETWORK_HTTP_MAX_CONCURRENT,
    };
    use crate::runtime::execution_context::RuntimeExecutionContext;
    use crate::runtime::llm::LlmRegistry;
    use serde_json::{Value, json};
    use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

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

    fn offline_deps() -> DispatcherDeps {
        DispatcherDeps {
            pool: MySqlPoolOptions::new().connect_lazy_with(MySqlConnectOptions::new()),
            s3: None,
            registry: Arc::new(CapabilityRegistry::new()),
            llm: Arc::new(LlmRegistry::new()),
            rate_limits: Arc::new(
                crate::runtime::capabilities::rate_limit::CapabilityRateLimits::new(),
            ),
        }
    }

    #[test]
    fn provider_text_containing_timeout_cannot_forge_timeout_classification() {
        let (reply, outcome, audit_error) = to_reply::<Value>(
            super::LLM_INVOKE,
            Err(CapabilityFailure::failed(
                "provider said timeout: PROVIDER_TIMEOUT_SENTINEL",
            )),
        );

        assert_eq!(reply.code, Some(5000));
        assert_eq!(
            reply.message.as_deref(),
            Some("LLM capability invocation failed")
        );
        assert_eq!(outcome, "error");
        assert_eq!(
            audit_error.map(|error| error.kind),
            Some("llm_invocation_failed")
        );
        assert!(
            !serde_json::to_string(&reply)
                .unwrap()
                .contains("PROVIDER_TIMEOUT_SENTINEL")
        );
    }

    #[test]
    fn typed_internal_timeout_uses_contract_code_and_safe_kind() {
        let (reply, outcome, audit_error) =
            to_reply::<Value>(NETWORK_HTTP, Err(CapabilityFailure::timeout()));

        assert_eq!(reply.code, Some(4081));
        assert_eq!(
            reply.message.as_deref(),
            Some("Capability invocation timed out")
        );
        assert_eq!(outcome, "error");
        assert_eq!(
            audit_error.map(|error| error.kind),
            Some("capability_timeout")
        );
    }

    #[test]
    fn dispatcher_maps_model_preset_unknown_to_contract_code_5007() {
        let (reply, outcome, audit_error) =
            to_reply::<Value>(LLM_INVOKE, Err(CapabilityFailure::model_preset_unknown()));

        assert_eq!(reply.code, Some(5007));
        assert_eq!(outcome, "error");
        assert_eq!(
            audit_error.map(|error| error.kind),
            Some("model_preset_unknown")
        );
    }

    #[test]
    fn oversized_host_call_reply_is_replaced_by_a_bounded_error() {
        let reply = ReplyEnvelope::ok(json!({
            "body": "x".repeat(HOST_CALL_MAX_BYTES)
        }));

        let (encoded, exceeded) = serialize_bounded_reply(reply);
        let parsed: Value = serde_json::from_str(&encoded).unwrap();

        assert!(exceeded);
        assert!(encoded.len() <= HOST_CALL_MAX_BYTES);
        assert_eq!(parsed["code"], 5000);
        assert_eq!(parsed["message"], "Capability response exceeds 4 MiB limit");
    }

    #[tokio::test]
    async fn dispatcher_rejects_oversized_host_call_input_before_handler_execution() {
        let deps = offline_deps();
        let ctx = DispatchCtx {
            execution_context: RuntimeExecutionContext::best_effort(
                Some("oversized-payload-test".into()),
                None,
            ),
            agent_id: 1,
            plugin_id: 2,
            function_id: None,
            permissions: vec!["time.now".into()],
        };
        let envelope = json!({
            "capability": "time.now",
            "args": {"padding": "x".repeat(HOST_CALL_MAX_BYTES)}
        })
        .to_string();

        let reply = dispatch(&deps, &ctx, &envelope).await;
        let parsed: Value = serde_json::from_str(&reply).unwrap();

        assert_eq!(parsed["code"], 4001);
        assert_eq!(parsed["message"], "Payload exceeds 4 MiB limit");
    }

    #[tokio::test]
    async fn semantic_network_argument_errors_use_the_invalid_args_code() {
        let deps = offline_deps();
        let ctx = DispatchCtx {
            execution_context: RuntimeExecutionContext::best_effort(
                Some("invalid-network-args-test".into()),
                None,
            ),
            agent_id: 1,
            plugin_id: 2,
            function_id: None,
            permissions: vec![NETWORK_HTTP.into()],
        };
        let envelope = json!({
            "capability": NETWORK_HTTP,
            "args": {
                "method": "TRACE",
                "url": "https://example.com"
            }
        })
        .to_string();

        let reply = dispatch(&deps, &ctx, &envelope).await;
        let parsed: Value = serde_json::from_str(&reply).unwrap();

        assert_eq!(parsed["code"], 4001);
        assert_eq!(parsed["message"], "Capability arguments were invalid");
    }

    #[tokio::test]
    async fn permission_database_failure_emits_exactly_one_safe_audit() {
        let writer = TraceWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let mut deps = offline_deps();
        deps.pool = MySqlPoolOptions::new()
            .acquire_timeout(Duration::from_millis(50))
            .connect_lazy_with(
                MySqlConnectOptions::new()
                    .host("127.0.0.1")
                    .port(1)
                    .username("PERMISSION_DB_SENTINEL"),
            );
        let ctx = DispatchCtx {
            execution_context: RuntimeExecutionContext::best_effort(
                Some("permission-audit-test".into()),
                Some(11),
            ),
            agent_id: 3,
            plugin_id: 7,
            function_id: Some(5),
            permissions: Vec::new(),
        };
        let envelope = json!({
            "capability": "time.now",
            "args": {"private": "PERMISSION_PAYLOAD_SENTINEL"}
        });

        let reply = dispatch(&deps, &ctx, &envelope.to_string()).await;
        let parsed: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(parsed["code"], 5000);
        assert_eq!(parsed["message"], "permission lookup failed");

        let output =
            String::from_utf8(writer.0.lock().expect("trace buffer poisoned").clone()).unwrap();
        assert_eq!(
            output.matches("event_type=\"capability_call\"").count(),
            1,
            "permission lookup failure must emit exactly one runtime audit: {output}"
        );
        assert!(output.contains("outcome=\"error\""));
        assert!(output.contains("error_kind=\"capability_handler_failed\""));
        for sentinel in ["PERMISSION_DB_SENTINEL", "PERMISSION_PAYLOAD_SENTINEL"] {
            assert!(
                !output.contains(sentinel),
                "permission failure audit leaked {sentinel}: {output}"
            );
        }
    }

    #[tokio::test]
    async fn dispatcher_tracing_omits_unknown_args_denied_bodies_and_raw_handler_errors() {
        let writer = TraceWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let deps = offline_deps();

        let denied_ctx = DispatchCtx {
            execution_context: RuntimeExecutionContext::best_effort(
                Some("safe-audit-test".into()),
                None,
            ),
            agent_id: 1,
            plugin_id: 2,
            function_id: Some(3),
            permissions: vec!["time.now".into()],
        };
        let unknown = json!({
            "capability": "unknown.UNKNOWN_CAPABILITY_SENTINEL",
            "args": {"body": "UNKNOWN_BODY_SENTINEL"}
        });
        let unknown_reply = dispatch(&deps, &denied_ctx, &unknown.to_string()).await;
        assert!(unknown_reply.contains("\"code\":4045"));
        assert!(!unknown_reply.contains("UNKNOWN_CAPABILITY_SENTINEL"));

        let invalid_envelope_reply =
            dispatch(&deps, &denied_ctx, r#"{"capability":"ENVELOPE_SENTINEL""#).await;
        assert!(invalid_envelope_reply.contains("Invalid host_call envelope"));
        assert!(!invalid_envelope_reply.contains("ENVELOPE_SENTINEL"));

        let denied_network = json!({
            "capability": NETWORK_HTTP,
            "args": {
                "method": "POST",
                "url": "https://example.com/private?token=QUERY_SENTINEL",
                "headers": {
                    "authorization": "AUTHORIZATION_SENTINEL",
                    "cookie": "COOKIE_SENTINEL"
                },
                "body": "BODY_SENTINEL"
            }
        });
        dispatch(&deps, &denied_ctx, &denied_network.to_string()).await;

        let fs_ctx = DispatchCtx {
            permissions: vec![FS_READ.into()],
            ..denied_ctx
        };
        let failing_fs = json!({
            "capability": FS_READ,
            "args": {
                "path": "ORIGINAL_DOWNSTREAM_ERROR_SENTINEL/missing.txt"
            }
        });
        let failing_reply = dispatch(&deps, &fs_ctx, &failing_fs.to_string()).await;
        let failing_reply_json: Value = serde_json::from_str(&failing_reply).unwrap();
        assert_eq!(failing_reply_json["code"], 5000);
        assert_eq!(
            failing_reply_json["message"],
            "Filesystem capability operation failed"
        );
        assert!(!failing_reply.contains("ORIGINAL_DOWNSTREAM_ERROR_SENTINEL"));

        let invalid_fs_args = json!({
            "capability": FS_READ,
            "args": {
                "path": {"private": "ARGS_ERROR_SENTINEL"}
            }
        });
        let invalid_args_reply = dispatch(&deps, &fs_ctx, &invalid_fs_args.to_string()).await;
        let invalid_args_reply_json: Value = serde_json::from_str(&invalid_args_reply).unwrap();
        assert_eq!(invalid_args_reply_json["code"], 4001);
        assert_eq!(
            invalid_args_reply_json["message"],
            "Invalid capability arguments"
        );
        assert!(!invalid_args_reply.contains("ARGS_ERROR_SENTINEL"));

        let output =
            String::from_utf8(writer.0.lock().expect("trace buffer poisoned").clone()).unwrap();
        assert!(output.contains("runtime_audit"));
        assert!(output.contains("filesystem_operation_failed"));
        assert!(output.contains("Filesystem capability operation failed"));
        for sentinel in [
            "UNKNOWN_CAPABILITY_SENTINEL",
            "UNKNOWN_BODY_SENTINEL",
            "ENVELOPE_SENTINEL",
            "QUERY_SENTINEL",
            "AUTHORIZATION_SENTINEL",
            "COOKIE_SENTINEL",
            "BODY_SENTINEL",
            "ORIGINAL_DOWNSTREAM_ERROR_SENTINEL",
            "ARGS_ERROR_SENTINEL",
        ] {
            assert!(
                !output.contains(sentinel),
                "trace leaked {sentinel}: {output}"
            );
        }
    }

    #[tokio::test]
    async fn dispatcher_rejects_ninth_network_call_with_safe_rate_limit_audit() {
        let writer = TraceWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let deps = offline_deps();
        let held = (0..NETWORK_HTTP_MAX_CONCURRENT)
            .map(|_| {
                deps.rate_limits
                    .try_acquire_network(7, Some(11))
                    .expect("the first eight calls must be admitted")
            })
            .collect::<Vec<_>>();
        let ctx = DispatchCtx {
            execution_context: RuntimeExecutionContext::best_effort(
                Some("network-rate-limit-test".into()),
                Some(11),
            ),
            agent_id: 3,
            plugin_id: 7,
            function_id: Some(5),
            permissions: vec![NETWORK_HTTP.into()],
        };
        let envelope = json!({
            "capability": NETWORK_HTTP,
            "args": {
                "method": "GET",
                "url": "https://RATE_LIMIT_URL_SENTINEL.example/private?token=QUERY_SENTINEL",
                "headers": {"authorization": "HEADER_SENTINEL"}
            }
        });

        let reply = dispatch(&deps, &ctx, &envelope.to_string()).await;
        let parsed: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(parsed["code"], 4292);
        assert_eq!(parsed["message"], "Capability rate limit exceeded");

        let output =
            String::from_utf8(writer.0.lock().expect("trace buffer poisoned").clone()).unwrap();
        assert_eq!(
            output.matches("event_type=\"capability_denied\"").count(),
            1,
            "a rate-limit rejection must emit exactly one denied audit: {output}"
        );
        assert!(output.contains("outcome=\"denied\""));
        assert!(output.contains("capability_rate_limited"));
        for sentinel in [
            "RATE_LIMIT_URL_SENTINEL",
            "QUERY_SENTINEL",
            "HEADER_SENTINEL",
        ] {
            assert!(
                !output.contains(sentinel),
                "rate-limit audit leaked {sentinel}: {output}"
            );
        }
        drop(held);
    }

    #[tokio::test]
    async fn dispatcher_rejects_immediate_hundred_and_first_log_without_running_handler() {
        let writer = TraceWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let deps = offline_deps();
        let fixed = Instant::now() + Duration::from_secs(1);
        for _ in 0..LOG_EMIT_MAX_PER_SECOND {
            assert!(deps.rate_limits.try_acquire_log_at(7, fixed));
        }
        let ctx = DispatchCtx {
            execution_context: RuntimeExecutionContext::best_effort(
                Some("log-rate-limit-test".into()),
                Some(11),
            ),
            agent_id: 3,
            plugin_id: 7,
            function_id: Some(5),
            permissions: vec![LOG_EMIT.into()],
        };
        let envelope = json!({
            "capability": LOG_EMIT,
            "args": {
                "level": "warn",
                "message": "LOG_RATE_LIMIT_SENTINEL",
                "fields": {"private": "FIELD_RATE_LIMIT_SENTINEL"}
            }
        });

        let reply = dispatch(&deps, &ctx, &envelope.to_string()).await;
        let parsed: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(parsed["code"], 4292);
        assert_eq!(parsed["message"], "Capability rate limit exceeded");

        let output =
            String::from_utf8(writer.0.lock().expect("trace buffer poisoned").clone()).unwrap();
        assert!(!output.contains("plugin_log_emit"));
        assert_eq!(
            output.matches("event_type=\"capability_denied\"").count(),
            1
        );
        assert!(output.contains("capability_rate_limited"));
        assert!(!output.contains("LOG_RATE_LIMIT_SENTINEL"));
        assert!(!output.contains("FIELD_RATE_LIMIT_SENTINEL"));
    }

    #[tokio::test]
    async fn invalid_log_arguments_do_not_consume_the_last_token() {
        let deps = offline_deps();
        let fixed = Instant::now() + Duration::from_secs(1);
        for _ in 0..(LOG_EMIT_MAX_PER_SECOND - 1) {
            assert!(deps.rate_limits.try_acquire_log_at(7, fixed));
        }
        let ctx = DispatchCtx {
            execution_context: RuntimeExecutionContext::best_effort(None, Some(11)),
            agent_id: 3,
            plugin_id: 7,
            function_id: Some(5),
            permissions: vec![LOG_EMIT.into()],
        };
        let invalid = json!({
            "capability": LOG_EMIT,
            "args": {"level": "warn"}
        });

        let reply = dispatch(&deps, &ctx, &invalid.to_string()).await;
        let parsed: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(parsed["code"], 4001);
        assert!(
            deps.rate_limits.try_acquire_log_at(7, fixed),
            "invalid args must not consume the final token"
        );
        assert!(!deps.rate_limits.try_acquire_log_at(7, fixed));
    }

    /// T080 §7 兼容性：HiveWeb host_call envelope 必须与共享
    /// `hive-runtime-core::abi::HostCallReply` 字节级等价（成功 + 共享错误码）。
    /// 任一字段顺序或转义不一致都会让 Plugin 侧解析到不同字节，故逐字节断言。
    #[test]
    fn plugin_host_call_envelope_is_byte_identical_to_shared_abi() {
        use hive_runtime_core::abi::{HostCallReply, StableErrorKind};

        // 成功路径：{"ok":true,"data":...}
        let ok_value = json!({"status": 200, "headers": {}, "body": "ok"});
        let web = serde_json::to_string(&ReplyEnvelope::ok(ok_value.clone())).expect("serialize");
        let shared = String::from_utf8(
            HostCallReply::success_json(&serde_json::to_vec(&ok_value).expect("to_vec"))
                .expect("valid JSON")
                .encode_json(),
        )
        .expect("utf8");
        assert_eq!(web, shared, "success envelope must be byte-identical");

        // 失败路径：{"ok":false,"code":<n>,"message":"..."}（共享错误码）
        let cases = [
            (
                4030u16,
                StableErrorKind::CapabilityDenied,
                "当前 Agent 未授权",
            ),
            (
                4045u16,
                StableErrorKind::CapabilityUnknown,
                "未知 Capability",
            ),
            (4001u16, StableErrorKind::InvalidArgs, "参数无效"),
            (4081u16, StableErrorKind::CapabilityTimeout, "调用超时"),
            (5000u16, StableErrorKind::Internal, "内部错误"),
        ];
        for (code, kind, msg) in cases {
            let web = serde_json::to_string(&ReplyEnvelope::err(code, msg)).expect("serialize");
            let shared =
                String::from_utf8(HostCallReply::failure(kind, msg).encode_json()).expect("utf8");
            assert_eq!(web, shared, "code {code} envelope must be byte-identical");
        }
    }
}
