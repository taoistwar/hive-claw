//! Plugin 调用入口（FR-006 / FR-029 / FR-030 / US4 commit 2）
//!
//! 流程：
//!   resolve plugin row → pool.acquire（命中 idle / 冷启动 + sha256 verify）→
//!   tokio::task::spawn_blocking 包 plugin.call_with_host_context →
//!   release (reset + 归还 / 失败丢弃) → audit + 返回。
//!
//! host_call host_fn 在编译期由 PluginBuilder.with_function 注入；执行期 Plugin
//! 通过 `host_call(envelope_json_str)` 同步调用，宿主 host_fn 拿到
//! `&mut Arc<HostInvocationCtx>`（来自 call_with_host_context）后用
//! `tokio::runtime::Handle::current().block_on` 桥到异步 dispatcher。

use aws_sdk_s3::Client as S3Client;
use extism::{
    CurrentPlugin, Error as ExtismError, Manifest, Plugin as ExtismPlugin, PluginBuilder, UserData,
    Val, Wasm,
};
use sqlx::MySqlPool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::models::Plugin as PluginRow;
use crate::runtime::capability::{self, CapabilityRegistry, DispatchCtx, DispatcherDeps};
use crate::runtime::pool::{InstancePool, PoolError, PooledPlugin};
use crate::services::runtime_audit::{self, AuditRecord};
use crate::utils::error::AppError;

/// 单次 host_call 时由 invoker 注入到 Plugin host_context 的上下文。
/// 包含调用元信息 + 全局共享 dispatcher deps（pool / s3 / registry）。
pub struct HostInvocationCtx {
    pub deps: DispatcherDeps,
    pub dispatch: DispatchCtx,
}

// host_call host_fn (raw form — `host_fn!` 宏因卫生原因不让用户访问 `plugin`，
// 我们需要 plugin.host_context::<T>() 取每次调用的 ctx；故手写展开)。
//
// Plugin 调用 `host_call(envelope: String) -> String`：
//   1. 从 wasm linear memory 读 envelope
//   2. 取 host_context (由 call_with_host_context 注入)
//   3. block_on 桥到 async dispatcher
//   4. 写返回字符串回 wasm memory
pub fn host_call(
    plugin: &mut CurrentPlugin,
    inputs: &[Val],
    outputs: &mut [Val],
    _user_data: UserData<()>,
) -> Result<(), ExtismError> {
    let envelope: String = plugin.memory_get_val(&inputs[0])?;
    let ctx = plugin.host_context::<Arc<HostInvocationCtx>>()?.clone();
    let result = tokio::runtime::Handle::current().block_on(async move {
        capability::dispatch(&ctx.deps, &ctx.dispatch, &envelope).await
    });
    let handle = plugin.memory_new(&result)?;
    if !outputs.is_empty() {
        outputs[0] = plugin.memory_to_val(handle);
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum InvokerError {
    #[error("plugin id={0} not found or deleted")]
    PluginMissing(i64),
    #[error("pool busy")]
    PoolBusy,
    #[error("timeout after {0}ms")]
    Timeout(u64),
    #[error("plugin runtime error: {0}")]
    PluginError(String),
    #[error("pool error: {0}")]
    Pool(#[from] PoolError),
}

impl From<InvokerError> for AppError {
    fn from(e: InvokerError) -> Self {
        match e {
            InvokerError::PluginMissing(_) => AppError::NotFound(e.to_string()),
            InvokerError::PoolBusy => AppError::PoolBusy("plugin pool busy".into()),
            InvokerError::Timeout(ms) => AppError::PluginInvocationTimeout(format!(
                "Plugin 执行超过 {ms} 毫秒已被中止"
            )),
            InvokerError::PluginError(m) => AppError::Internal(m),
            InvokerError::Pool(m) => AppError::Internal(m.to_string()),
        }
    }
}

#[derive(Debug)]
pub struct Invoker {
    pub pool: Arc<InstancePool>,
}

impl Invoker {
    pub fn new(pool: Arc<InstancePool>) -> Self {
        Self { pool }
    }

    /// 调用 Plugin 导出函数 `export_name(json_str)` → 返回 json 字符串。
    /// 内部完成 acquire → call_with_host_context → reset → release + audit。
    #[allow(clippy::too_many_arguments)]
    pub async fn invoke(
        &self,
        db_pool: &MySqlPool,
        s3: &S3Client,
        registry: Arc<CapabilityRegistry>,
        llm: Arc<crate::runtime::llm::LlmRegistry>,
        plugin_id: i64,
        export_name: &str,
        input_json: String,
        dispatch_ctx: DispatchCtx,
    ) -> Result<String, InvokerError> {
        let deps = DispatcherDeps {
            pool: db_pool.clone(),
            s3: s3.clone(),
            registry,
            llm,
        };
        let t0 = Instant::now();

        // 1. resolve plugin row
        let row: Option<PluginRow> = sqlx::query_as(
            "SELECT * FROM plugins WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(plugin_id)
        .fetch_optional(db_pool)
        .await
        .map_err(|e| InvokerError::PluginError(format!("plugin lookup: {e}")))?;
        let row = row.ok_or(InvokerError::PluginMissing(plugin_id))?;

        // 2. 准备 host_call 构建闭包（spawn_blocking 内执行）
        let timeout_ms = self.pool.config.call_timeout_ms;
        let memory_mb = self.pool.config.call_memory_mb;
        let build = move |bytes: Vec<u8>| -> Result<ExtismPlugin, anyhow::Error> {
            let manifest = Manifest::new([Wasm::data(bytes)])
                .with_timeout(Duration::from_millis(timeout_ms))
                .with_memory_max((memory_mb * 1024 * 1024) as u32);
            let plugin = PluginBuilder::new(manifest)
                .with_wasi(true)
                .with_function(
                    "host_call",
                    [extism::ValType::I64],
                    [extism::ValType::I64],
                    UserData::default(),
                    host_call,
                )
                .build()?;
            Ok(plugin)
        };

        // 3. acquire from pool
        let mut inst: PooledPlugin = self.pool.acquire(s3, &row, build).await?;

        // 4. assemble host context for this invocation
        let host_ctx = Arc::new(HostInvocationCtx {
            deps,
            dispatch: dispatch_ctx,
        });

        // 5. spawn_blocking 包 plugin.call_with_host_context
        let export = export_name.to_string();
        let input = input_json;
        let plugin_id_local = plugin_id;
        let call_result = tokio::task::spawn_blocking(move || {
            let res = inst
                .plugin
                .call_with_host_context::<&str, String, _>(&export, input.as_str(), host_ctx);
            // 调用后尝试 reset（FR-029）— 成功才能复用
            let reset_ok = inst.plugin.reset().is_ok();
            (res, inst, reset_ok)
        })
        .await;

        let (call_res, returned_inst, reset_ok) = match call_result {
            Ok(t) => t,
            Err(e) => {
                return Err(InvokerError::PluginError(format!(
                    "spawn_blocking join: {e}"
                )))
            }
        };

        // 6. release instance back to pool
        self.pool.release(returned_inst, reset_ok).await;

        let elapsed_ms = t0.elapsed().as_millis() as i32;

        // 7. audit plugin_invoke event
        let (outcome, err_msg, output) = match call_res {
            Ok(out) => ("success", None, Some(out)),
            Err(e) => {
                let msg = format!("{e}");
                let is_timeout = msg.contains("timeout") || msg.contains("fuel");
                (
                    if is_timeout { "timeout" } else { "error" },
                    Some(msg),
                    None,
                )
            }
        };

        runtime_audit::record(
            db_pool,
            AuditRecord {
                request_id: None,
                session_id: None,
                agent_id: None,
                plugin_id: Some(plugin_id_local),
                function_id: None,
                capability: None,
                event_type: "plugin_invoke",
                outcome,
                elapsed_ms: Some(elapsed_ms),
                error_message: err_msg.as_deref(),
                payload_summary: None,
            },
        )
        .await;

        match (output, outcome) {
            (Some(s), _) => Ok(s),
            (None, "timeout") => Err(InvokerError::Timeout(timeout_ms)),
            (None, _) => Err(InvokerError::PluginError(
                err_msg.unwrap_or_else(|| "unknown plugin error".into()),
            )),
        }
    }
}
