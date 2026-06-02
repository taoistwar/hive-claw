//! Capability 静态注册表 + Dispatcher (FR-001 / FR-003 / data-model §6)
//!
//! Capability 是宿主侧零信任默认 deny 的能力点；Agent 的 permissions 必须显式
//! 列出某 capability 才允许 Plugin 在调用时使用它。注册表 = 代码侧真值源；DB
//! `capabilities` 表 = 启动期 upsert 的镜像（V008 + V018 seed）。
//!
//! Dispatcher（T102）：
//!   Plugin host_call(envelope) -> 解析 -> 鉴权 (Agent.permissions) ->
//!   未知 capability 返 4045 -> handler -> audit -> 返回 envelope。

use aws_sdk_s3::Client as S3Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::MySqlPool;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use crate::runtime::capabilities;
use crate::services::runtime_audit::{self, AuditRecord};

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
pub const CHAT_RESPOND: &str = "chat.respond";
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
    pub request_id: Option<String>,
    pub session_id: Option<i64>,
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
    pub s3: S3Client,
    pub registry: Arc<CapabilityRegistry>,
    pub llm: Arc<crate::runtime::llm::LlmRegistry>,
}

impl std::fmt::Debug for DispatcherDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatcherDeps").finish_non_exhaustive()
    }
}

/// Dispatcher 主入口。返回的 JSON 字符串会被 Plugin 侧解码成 ReplyEnvelope。
pub async fn dispatch(deps: &DispatcherDeps, ctx: &DispatchCtx, envelope_str: &str) -> String {
    let pool = &deps.pool;
    let registry = &deps.registry;
    let t0 = Instant::now();
    let request_id = ctx.request_id.as_deref();

    let parsed: Result<CallEnvelope, _> = serde_json::from_str(envelope_str);
    let envelope = match parsed {
        Ok(e) => e,
        Err(e) => {
            let reply = ReplyEnvelope::err(4000, format!("invalid host_call envelope: {e}"));
            runtime_audit::record(
                pool,
                AuditRecord {
                    request_id,
                    session_id: ctx.session_id,
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
            )
            .await;
            return serde_json::to_string(&reply).unwrap_or_default();
        }
    };

    // 1. unknown capability → 4045
    let cap_name = envelope.capability.clone();
    if registry.lookup(&cap_name).is_none() {
        let reply = ReplyEnvelope::err(4045, format!("unknown capability: {cap_name}"));
        runtime_audit::record(
            pool,
            AuditRecord {
                request_id,
                session_id: ctx.session_id,
                agent_id: Some(ctx.agent_id),
                plugin_id: Some(ctx.plugin_id),
                function_id: ctx.function_id,
                capability: Some(&cap_name),
                event_type: "capability_denied",
                outcome: "denied",
                elapsed_ms: Some(t0.elapsed().as_millis() as i32),
                error_message: Some("unknown capability"),
                payload_summary: Some(runtime_audit::redact_args(&envelope.args)),
            },
        )
        .await;
        return serde_json::to_string(&reply).unwrap_or_default();
    }

    // 2. permission check
    let granted: HashSet<String> = if !ctx.permissions.is_empty() {
        ctx.permissions.iter().cloned().collect()
    } else {
        match load_agent_permissions(pool, ctx.agent_id).await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, agent_id = ctx.agent_id, "load_agent_permissions");
                let reply = ReplyEnvelope::err(5000, "permission lookup failed");
                return serde_json::to_string(&reply).unwrap_or_default();
            }
        }
    };
    if !granted.contains(&cap_name) {
        let reply = ReplyEnvelope::err(4030, format!("当前 Agent 未授权调用能力「{cap_name}」"));
        runtime_audit::record(
            pool,
            AuditRecord {
                request_id,
                session_id: ctx.session_id,
                agent_id: Some(ctx.agent_id),
                plugin_id: Some(ctx.plugin_id),
                function_id: ctx.function_id,
                capability: Some(&cap_name),
                event_type: "capability_denied",
                outcome: "denied",
                elapsed_ms: Some(t0.elapsed().as_millis() as i32),
                error_message: Some("capability not granted"),
                payload_summary: Some(runtime_audit::redact_args(&envelope.args)),
            },
        )
        .await;
        return serde_json::to_string(&reply).unwrap_or_default();
    }

    // 3. handler dispatch
    // helper closure: 把 Result<T:Serialize, String> 转成 (ReplyEnvelope, outcome, err_msg)
    fn to_reply<T: Serialize>(
        r: Result<T, String>,
    ) -> (ReplyEnvelope, &'static str, Option<String>) {
        match r {
            Ok(v) => (
                ReplyEnvelope::ok(serde_json::to_value(v).unwrap_or(Value::Null)),
                "success",
                None,
            ),
            Err(e) => (ReplyEnvelope::err(4000, e.clone()), "error", Some(e)),
        }
    }
    fn args_err(
        label: &str,
        e: serde_json::Error,
    ) -> (ReplyEnvelope, &'static str, Option<String>) {
        (
            ReplyEnvelope::err(4000, format!("{label} args: {e}")),
            "error",
            Some("invalid args".to_string()),
        )
    }

    let (reply, outcome, err_msg) = match cap_name.as_str() {
        TIME_NOW => (
            ReplyEnvelope::ok(capabilities::utility::time_now()),
            "success",
            None,
        ),
        LOG_EMIT => match serde_json::from_value(envelope.args.clone()) {
            Ok(args) => {
                let data =
                    capabilities::utility::log_emit(args, Some(ctx.plugin_id), Some(ctx.agent_id));
                (ReplyEnvelope::ok(data), "success", None)
            }
            Err(e) => args_err("log.emit", e),
        },
        FS_READ => match serde_json::from_value(envelope.args.clone()) {
            Ok(args) => to_reply(capabilities::fs::fs_read(args).await),
            Err(e) => args_err("fs.read", e),
        },
        FS_WRITE => match serde_json::from_value(envelope.args.clone()) {
            Ok(args) => to_reply(capabilities::fs::fs_write(args).await),
            Err(e) => args_err("fs.write", e),
        },
        NETWORK_HTTP => match serde_json::from_value(envelope.args.clone()) {
            Ok(args) => to_reply(capabilities::network_http::http_request(args).await),
            Err(e) => args_err("network.http", e),
        },
        S3_READ => match serde_json::from_value(envelope.args.clone()) {
            Ok(args) => to_reply(capabilities::s3::s3_read(&deps.s3, args).await),
            Err(e) => args_err("s3.read", e),
        },
        S3_WRITE => match serde_json::from_value(envelope.args.clone()) {
            Ok(args) => to_reply(capabilities::s3::s3_write(&deps.s3, args).await),
            Err(e) => args_err("s3.write", e),
        },
        SECRET_GET => match serde_json::from_value(envelope.args.clone()) {
            Ok(args) => to_reply(capabilities::secret::secret_get(args)),
            Err(e) => args_err("secret.get", e),
        },
        DB_QUERY => match serde_json::from_value(envelope.args.clone()) {
            Ok(args) => to_reply(capabilities::db::db_query(&deps.pool, args).await),
            Err(e) => args_err("db.query", e),
        },
        DB_EXECUTE => match serde_json::from_value(envelope.args.clone()) {
            Ok(args) => to_reply(capabilities::db::db_execute(&deps.pool, args).await),
            Err(e) => args_err("db.execute", e),
        },
        LLM_INVOKE => match serde_json::from_value(envelope.args.clone()) {
            Ok(args) => to_reply(
                capabilities::llm::llm_invoke(&deps.pool, &deps.llm, ctx.agent_id, args).await,
            ),
            Err(e) => args_err("llm.invoke", e),
        },
        _ => (
            ReplyEnvelope::err(5001, format!("unknown handler for {cap_name}")),
            "error",
            Some("handler unimplemented".to_string()),
        ),
    };

    runtime_audit::record(
        pool,
        AuditRecord {
            request_id,
            session_id: ctx.session_id,
            agent_id: Some(ctx.agent_id),
            plugin_id: Some(ctx.plugin_id),
            function_id: ctx.function_id,
            capability: Some(&cap_name),
            event_type: "capability_call",
            outcome,
            elapsed_ms: Some(t0.elapsed().as_millis() as i32),
            error_message: err_msg.as_deref(),
            payload_summary: Some(json!({
                "args": runtime_audit::redact_args(&envelope.args),
                "ok": reply.ok,
            })),
        },
    )
    .await;

    serde_json::to_string(&reply).unwrap_or_default()
}
