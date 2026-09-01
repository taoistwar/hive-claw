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
    CancelHandle, CompiledPlugin, CurrentPlugin, Error as ExtismError, Manifest, PluginBuilder,
    UserData, Val, Wasm,
};
use sqlx::MySqlPool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hive_runtime_core::{
    abi::HIVE_EXTISM_ABI_V1,
    wasm::{
        ABI_VERSION_EXPORT, HOST_CALL_IMPORT, WasmModuleShape, WasmValidationError,
        validate_wasm_shape,
    },
};

use crate::models::Plugin as PluginRow;
use crate::runtime::capability::{self, CapabilityRegistry, DispatchCtx, DispatcherDeps};
use crate::runtime::execution_context::RuntimeExecutionContext;
use crate::runtime::pool::{InstancePool, PoolError, PooledPlugin};
use crate::services::runtime_audit::{self, AuditRecord};
use crate::utils::error::AppError;

const WASM_PAGES_PER_MIB: u64 = 16;

fn memory_limit_pages(memory_mb: u64) -> anyhow::Result<u32> {
    if memory_mb == 0 {
        anyhow::bail!("PLUGIN_CALL_MAX_MEMORY_MB must be greater than zero");
    }

    let pages = memory_mb
        .checked_mul(WASM_PAGES_PER_MIB)
        .ok_or_else(|| anyhow::anyhow!("PLUGIN_CALL_MAX_MEMORY_MB is too large"))?;
    u32::try_from(pages)
        .map_err(|_| anyhow::anyhow!("PLUGIN_CALL_MAX_MEMORY_MB exceeds the Extism page limit"))
}

#[cfg(test)]
fn build_extism_plugin(
    bytes: Vec<u8>,
    timeout_ms: u64,
    memory_mb: u64,
    fuel: u64,
) -> anyhow::Result<extism::Plugin> {
    let compiled = build_extism_compiled_plugin(bytes, timeout_ms, memory_mb, fuel)?;
    extism::Plugin::new_from_compiled(&compiled)
}

fn build_extism_compiled_plugin(
    bytes: Vec<u8>,
    timeout_ms: u64,
    memory_mb: u64,
    fuel: u64,
) -> anyhow::Result<CompiledPlugin> {
    if fuel == 0 {
        anyhow::bail!("PLUGIN_CALL_FUEL must be greater than zero");
    }

    // Extism's `with_memory_max` takes 64 KiB WebAssembly pages, not bytes.
    let memory_pages = memory_limit_pages(memory_mb)?;
    let manifest = Manifest::new([Wasm::data(bytes)])
        .with_timeout(Duration::from_millis(timeout_ms))
        .with_memory_max(memory_pages);
    let compiled = PluginBuilder::new(manifest)
        .with_wasi(false)
        .with_fuel_limit(fuel)
        .with_function(
            HOST_CALL_IMPORT,
            [extism::ValType::I64],
            [extism::ValType::I64],
            UserData::default(),
            host_call,
        )
        .compile()?;
    Ok(compiled)
}

fn shared_wasm_rejection_error(
    kind: WasmModuleShape,
    validation: WasmValidationError,
) -> anyhow::Error {
    anyhow::anyhow!("{kind:?}: {validation}")
}

/// Production builder: validate the bytes fetched for this invocation before
/// compiling them. Test-only resource probes may use the unchecked builder
/// above because their synthetic modules intentionally omit the Plugin ABI.
fn build_validated_extism_compiled_plugin(
    bytes: Vec<u8>,
    timeout_ms: u64,
    memory_mb: u64,
    fuel: u64,
) -> anyhow::Result<CompiledPlugin> {
    let validation = validate_wasm_shape(&bytes, false);
    if let Some(kind) = validation.rejection_kind() {
        return Err(shared_wasm_rejection_error(kind, validation));
    }

    build_extism_compiled_plugin(bytes, timeout_ms, memory_mb, fuel)
}

fn unsupported_abi_version_error() -> ExtismError {
    shared_wasm_rejection_error(
        WasmModuleShape::UnsupportedAbiVersion,
        WasmValidationError::rejected(WasmModuleShape::UnsupportedAbiVersion, None),
    )
}

/// Query the shared Plugin ABI contract before dispatching a business export.
///
/// Static validation guarantees the function export exists. Its returned
/// value can only be verified on a live instance, so every invocation crosses
/// this gate before the caller-provided business closure is allowed to run.
fn call_after_abi_check<T>(
    plugin: &mut extism::Plugin,
    business_call: impl FnOnce(&mut extism::Plugin) -> Result<T, ExtismError>,
) -> Result<T, ExtismError> {
    // Missing exports, bad signatures, traps and invalid String encodings all
    // describe a Plugin that cannot satisfy the shared ABI-version contract.
    // Do not expose Extism's raw error text across the runtime boundary.
    let version = plugin
        .call::<_, String>(ABI_VERSION_EXPORT, "{}")
        .map_err(|_| unsupported_abi_version_error())?;
    if version != HIVE_EXTISM_ABI_V1 {
        return Err(unsupported_abi_version_error());
    }

    business_call(plugin)
}

fn plugin_error_is_timeout(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("timeout") || normalized.contains("fuel")
}

#[derive(Debug)]
enum BlockingCallError {
    WallClockTimeout,
    Join(tokio::task::JoinError),
}

async fn await_blocking_call<T, C>(
    pool: &InstancePool,
    plugin_id: i64,
    timeout_ms: u64,
    task: tokio::task::JoinHandle<T>,
    cancel: C,
) -> Result<T, BlockingCallError>
where
    C: FnOnce() -> Result<(), ExtismError> + Send,
{
    match tokio::time::timeout(Duration::from_millis(timeout_ms), task).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => {
            pool.discard_lost(plugin_id).await;
            Err(BlockingCallError::Join(error))
        }
        Err(_) => {
            // Extism cancellation advances Wasmtime's epoch, which interrupts
            // executing Wasm (including CPU loops). It cannot preempt arbitrary
            // synchronous Rust code while a host handler is blocked; capability
            // I/O handlers must therefore enforce their own operation timeouts.
            if cancel().is_err() {
                tracing::warn!(
                    plugin_id,
                    error_kind = "plugin_cancel_signal_failed",
                    "failed to signal cancellation for timed-out Plugin"
                );
            }
            // The outer caller returns at its wall-clock deadline regardless.
            // Dropping the JoinHandle detaches the blocking task, whose closure
            // exclusively owns the instance and has no path back to `release`.
            // Reconcile counters now; the task drops the instance once the
            // Wasm call (and any in-flight host handler) actually exits.
            pool.discard_lost(plugin_id).await;
            Err(BlockingCallError::WallClockTimeout)
        }
    }
}

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
//   3. block_on 桥到 async dispatcher（各 capability I/O 必须自行设置超时；
//      Extism epoch cancellation只能中止 Wasm，不能抢占正在执行的 Rust host handler）
//   4. 写返回字符串回 wasm memory
pub fn host_call(
    plugin: &mut CurrentPlugin,
    inputs: &[Val],
    outputs: &mut [Val],
    _user_data: UserData<()>,
) -> Result<(), ExtismError> {
    let envelope: String = plugin.memory_get_val(&inputs[0])?;
    let ctx = plugin.host_context::<Arc<HostInvocationCtx>>()?.clone();
    let result = tokio::runtime::Handle::current()
        .block_on(async move { capability::dispatch(&ctx.deps, &ctx.dispatch, &envelope).await });
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
    Pool(PoolError),
}

impl From<PoolError> for InvokerError {
    fn from(error: PoolError) -> Self {
        match error {
            PoolError::Busy => Self::PoolBusy,
            error => Self::Pool(error),
        }
    }
}

impl From<InvokerError> for AppError {
    fn from(e: InvokerError) -> Self {
        match e {
            InvokerError::PluginMissing(_) => AppError::NotFound(e.to_string()),
            InvokerError::PoolBusy => AppError::PoolBusy("plugin pool busy".into()),
            InvokerError::Timeout(ms) => {
                AppError::PluginInvocationTimeout(format!("Plugin 执行超过 {ms} 毫秒已被中止"))
            }
            InvokerError::PluginError(_) => {
                AppError::Internal("Plugin 执行失败或超过内存上限".into())
            }
            InvokerError::Pool(_) => AppError::Internal("Plugin 加载失败".into()),
        }
    }
}

#[derive(Debug)]
pub struct Invoker {
    pub pool: Arc<InstancePool>,
}

/// Exactly-once audit guard for the whole Plugin invocation lifecycle.
///
/// It is created before any fallible DB/pool/load work. Every explicit terminal
/// path finalizes it, while `Drop` safely covers `?`, cancellation and panic
/// paths without retaining raw runtime errors or Plugin input.
struct PluginInvokeAuditGuard {
    execution_context: RuntimeExecutionContext,
    agent_id: i64,
    plugin_id: i64,
    function_id: Option<i64>,
    started_at: Instant,
    completed: bool,
}

impl PluginInvokeAuditGuard {
    fn new(
        execution_context: RuntimeExecutionContext,
        agent_id: i64,
        plugin_id: i64,
        function_id: Option<i64>,
        started_at: Instant,
    ) -> Self {
        Self {
            execution_context,
            agent_id,
            plugin_id,
            function_id,
            started_at,
            completed: false,
        }
    }

    fn finish(&mut self, outcome: &'static str, failed: bool) {
        if self.completed {
            return;
        }
        self.completed = true;
        runtime_audit::record(
            &self.execution_context,
            AuditRecord {
                agent_id: Some(self.agent_id),
                plugin_id: Some(self.plugin_id),
                function_id: self.function_id,
                capability: None,
                event_type: "plugin_invoke",
                outcome,
                elapsed_ms: Some(self.started_at.elapsed().as_millis() as i32),
                error_message: failed.then_some("plugin invocation failed"),
                payload_summary: None,
            },
        );
    }
}

impl Drop for PluginInvokeAuditGuard {
    fn drop(&mut self) {
        self.finish("error", true);
    }
}

/// Owns the pool accounting for a checked-out Plugin while its blocking call
/// is in flight.
///
/// Normal completion explicitly disarms this guard after `release` (or after
/// `await_blocking_call` has already reconciled a timeout/join failure).
/// Dropping the parent invocation future instead cancels the guest and schedules
/// an async counter reconciliation. The detached blocking task owns and drops
/// the instance, so the cancellation path never puts it back into the idle pool.
struct InvocationPoolSlotGuard {
    pool: Arc<InstancePool>,
    plugin_id: i64,
    cancel_handle: CancelHandle,
    armed: bool,
}

impl InvocationPoolSlotGuard {
    fn new(pool: Arc<InstancePool>, plugin_id: i64, cancel_handle: CancelHandle) -> Self {
        Self {
            pool,
            plugin_id,
            cancel_handle,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for InvocationPoolSlotGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        if self.cancel_handle.cancel().is_err() {
            tracing::warn!(
                plugin_id = self.plugin_id,
                error_kind = "plugin_cancel_signal_failed",
                "failed to signal cancellation for aborted Plugin invocation"
            );
        }

        let pool = Arc::clone(&self.pool);
        let plugin_id = self.plugin_id;
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    pool.discard_lost(plugin_id).await;
                });
            }
            Err(_) => {
                tracing::error!(
                    plugin_id,
                    error_kind = "plugin_pool_slot_cleanup_unavailable",
                    "cannot reconcile aborted Plugin slot outside a Tokio runtime"
                );
            }
        }
    }
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
        s3: Option<&S3Client>,
        registry: Arc<CapabilityRegistry>,
        llm: Arc<crate::runtime::llm::LlmRegistry>,
        plugin_id: i64,
        export_name: &str,
        input_json: String,
        dispatch_ctx: DispatchCtx,
    ) -> Result<String, InvokerError> {
        let t0 = Instant::now();
        let mut audit = PluginInvokeAuditGuard::new(
            dispatch_ctx.execution_context.clone(),
            dispatch_ctx.agent_id,
            plugin_id,
            dispatch_ctx.function_id,
            t0,
        );

        // 1. resolve plugin row
        let row: Option<PluginRow> =
            sqlx::query_as("SELECT * FROM plugins WHERE id = ? AND deleted_at IS NULL")
                .bind(plugin_id)
                .fetch_optional(db_pool)
                .await
                .map_err(|e| InvokerError::PluginError(format!("plugin lookup: {e}")))?;
        let row = row.ok_or(InvokerError::PluginMissing(plugin_id))?;

        self.invoke_resolved(
            db_pool,
            s3,
            registry,
            llm,
            row,
            export_name,
            input_json,
            dispatch_ctx,
            &mut audit,
        )
        .await
    }

    /// Execute the pool-owning part of an invocation after the Plugin row has
    /// been resolved. No pool slot exists before this boundary.
    #[allow(clippy::too_many_arguments)]
    async fn invoke_resolved(
        &self,
        db_pool: &MySqlPool,
        s3: Option<&S3Client>,
        registry: Arc<CapabilityRegistry>,
        llm: Arc<crate::runtime::llm::LlmRegistry>,
        row: PluginRow,
        export_name: &str,
        input_json: String,
        dispatch_ctx: DispatchCtx,
        audit: &mut PluginInvokeAuditGuard,
    ) -> Result<String, InvokerError> {
        let plugin_id = row.id;
        let deps = DispatcherDeps {
            pool: db_pool.clone(),
            s3: s3.cloned(),
            registry,
            llm,
            rate_limits: crate::runtime::capabilities::rate_limit::global(),
        };

        // 2. 准备 host_call 构建闭包（spawn_blocking 内执行）
        let timeout_ms = self.pool.config.call_timeout_ms;
        let memory_mb = self.pool.config.call_memory_mb;
        let fuel = self.pool.config.call_fuel;
        let build = move |bytes: Vec<u8>| -> Result<CompiledPlugin, anyhow::Error> {
            build_validated_extism_compiled_plugin(bytes, timeout_ms, memory_mb, fuel)
        };

        // 3. acquire from pool
        let mut inst: PooledPlugin = self.pool.acquire(s3, &row, build).await?;

        // 4. assemble host context for this invocation
        let host_ctx = Arc::new(HostInvocationCtx {
            deps,
            dispatch: dispatch_ctx,
        });

        // 5. spawn_blocking: verify the live ABI value, then call the business export.
        let export = export_name.to_string();
        let input = input_json;
        let plugin_id_local = plugin_id;
        let cancel_handle = inst.plugin.cancel_handle();
        let mut slot_guard = InvocationPoolSlotGuard::new(
            Arc::clone(&self.pool),
            plugin_id_local,
            cancel_handle.clone(),
        );
        let call_task = tokio::task::spawn_blocking(move || {
            let res = call_after_abi_check(&mut inst.plugin, |plugin| {
                plugin.call_with_host_context::<&str, String, _>(&export, input.as_str(), host_ctx)
            });
            // The live Store/Instance is dropped after this invocation. The
            // pool retains only the immutable CompiledPlugin template.
            (res, inst, true)
        });
        let call_result = await_blocking_call(
            &self.pool,
            plugin_id_local,
            timeout_ms,
            call_task,
            move || cancel_handle.cancel(),
        )
        .await;
        if call_result.is_err() {
            // `await_blocking_call` reconciles timeout and JoinError paths
            // before returning them, so Drop must not decrement a second time.
            slot_guard.disarm();
        }

        let (call_res, returned_inst, reset_ok) = match call_result {
            Ok(result) => result,
            Err(BlockingCallError::WallClockTimeout) => {
                audit.finish("timeout", true);
                return Err(InvokerError::Timeout(timeout_ms));
            }
            Err(BlockingCallError::Join(error)) => {
                let message = format!("spawn_blocking join: {error}");
                audit.finish("error", true);
                return Err(InvokerError::PluginError(message));
            }
        };

        // 6. Only a successful invocation with a successful reset is reusable.
        // A trapped/OOM instance is discarded even when Extism reports that
        // reset itself succeeded, because its post-trap state is not trusted.
        let invocation_ok = call_res.is_ok();
        self.pool
            .release(returned_inst, reset_ok, invocation_ok)
            .await;
        slot_guard.disarm();

        // 7. audit plugin_invoke event
        let (outcome, err_msg, output) = match call_res {
            Ok(out) => ("success", None, Some(out)),
            Err(e) => {
                let msg = format!("{e}");
                let is_timeout = plugin_error_is_timeout(&msg);
                (
                    if is_timeout { "timeout" } else { "error" },
                    Some(msg),
                    None,
                )
            }
        };

        finalize_plugin_invocation(audit, output, outcome, err_msg, reset_ok, timeout_ms)
    }
}

fn finalize_plugin_invocation(
    audit: &mut PluginInvokeAuditGuard,
    output: Option<String>,
    outcome: &'static str,
    err_msg: Option<String>,
    reset_ok: bool,
    timeout_ms: u64,
) -> Result<String, InvokerError> {
    let audit_outcome = if !reset_ok && outcome == "success" {
        "error"
    } else {
        outcome
    };
    audit.finish(audit_outcome, !reset_ok || err_msg.is_some());

    match (output, outcome) {
        (Some(output), _) => Ok(output),
        (None, "timeout") => Err(InvokerError::Timeout(timeout_ms)),
        (None, _) => Err(InvokerError::PluginError(
            err_msg.unwrap_or_else(|| "unknown plugin error".into()),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BlockingCallError, HostInvocationCtx, Invoker, InvokerError, PluginInvokeAuditGuard,
        await_blocking_call, build_extism_compiled_plugin, build_extism_plugin,
        build_validated_extism_compiled_plugin, call_after_abi_check, finalize_plugin_invocation,
        memory_limit_pages, plugin_error_is_timeout,
    };
    use crate::models::Plugin as PluginRow;
    use crate::runtime::capabilities::rate_limit::CapabilityRateLimits;
    use crate::runtime::capability::{
        CapabilityRegistry, DispatchCtx, DispatcherDeps, LOG_EMIT, TIME_NOW,
    };
    use crate::runtime::execution_context::RuntimeExecutionContext;
    use crate::runtime::llm::LlmRegistry;
    use crate::runtime::pool::{InstancePool, PoolConfig, PoolError};
    use crate::utils::error::{AppError, codes};
    use sha2::{Digest, Sha256};
    use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    const MEMORY_LIMIT_PROBE_WAT: &str = r#"
        (module
          (memory (export "memory") 1)

          ;; Grow from one page to exactly 128 MiB, then write its last byte.
          (func (export "allocate_within_limit")
            i32.const 2047
            memory.grow
            i32.const -1
            i32.eq
            if
              unreachable
            end
            i32.const 134217727
            i32.const 1
            i32.store8)

          ;; Request exactly 129 MiB. If the 128 MiB runtime limit rejects the
          ;; grow, this write at the first byte beyond the limit produces a
          ;; genuine linear-memory bounds trap rather than a synthetic
          ;; `unreachable` trap.
          (func (export "allocate_over_limit")
            i32.const 2063
            memory.grow
            drop
            i32.const 134217728
            i32.const 1
            i32.store8))
    "#;

    const CPU_LOOP_WAT: &str = r#"
        (module
          (import "extism:host/env" "alloc"
            (func $abi_alloc (param i64) (result i64)))
          (import "extism:host/env" "store_u8"
            (func $abi_store_u8 (param i64 i32)))
          (import "extism:host/env" "output_set"
            (func $abi_output_set (param i64 i64)))
          (memory (export "memory") 1)
          (func (export "_hive_plugin_abi_version") (result i32)
            (local $abi_ptr i64)
            (local.set $abi_ptr (call $abi_alloc (i64.const 14)))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 0)) (i32.const 104))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 1)) (i32.const 105))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 2)) (i32.const 118))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 3)) (i32.const 101))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 4)) (i32.const 45))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 5)) (i32.const 101))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 6)) (i32.const 120))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 7)) (i32.const 116))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 8)) (i32.const 105))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 9)) (i32.const 115))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 10)) (i32.const 109))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 11)) (i32.const 47))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 12)) (i32.const 118))
            (call $abi_store_u8 (i64.add (local.get $abi_ptr) (i64.const 13)) (i32.const 49))
            (call $abi_output_set (local.get $abi_ptr) (i64.const 14))
            i32.const 0)
          (func (export "spin")
            (loop $forever
              br $forever)))
    "#;

    const HOST_CALL_ROUNDTRIP_WAT: &str = r#"
        (module
          (import "extism:host/env" "input_offset"
            (func $input_offset (result i64)))
          (import "extism:host/env" "length"
            (func $length (param i64) (result i64)))
          (import "extism:host/env" "output_set"
            (func $output_set (param i64 i64)))
          (import "extism:host/user" "host_call"
            (func $host_call (param i64) (result i64)))

          (func (export "run") (result i32)
            (local $reply i64)
            (local.set $reply (call $host_call (call $input_offset)))
            (call $output_set
              (local.get $reply)
              (call $length (local.get $reply)))
            i32.const 0))
    "#;

    const SHARED_SMOKE_WASM: &[u8] =
        include_bytes!("../../../hivegui/tests/fixtures/plugins/shared-smoke/plugin.wasm");

    fn shared_smoke_with_unsupported_abi_version() -> Vec<u8> {
        const SUPPORTED: &[u8] = b"hive-extism/v1";
        const UNSUPPORTED: &[u8] = b"hive-extism/v2";
        assert_eq!(SUPPORTED.len(), UNSUPPORTED.len());

        let mut bytes = SHARED_SMOKE_WASM.to_vec();
        let matches = bytes
            .windows(SUPPORTED.len())
            .enumerate()
            .filter_map(|(offset, candidate)| (candidate == SUPPORTED).then_some(offset))
            .collect::<Vec<_>>();
        assert_eq!(
            matches.len(),
            1,
            "the shared fixture must carry exactly one ABI version literal"
        );
        let offset = matches[0];
        bytes[offset..offset + SUPPORTED.len()].copy_from_slice(UNSUPPORTED);
        bytes
    }

    const WASI_IMPORT_WAT: &str = r#"
        (module
          (import "wasi_snapshot_preview1" "random_get"
            (func $random_get (param i32 i32) (result i32)))
          (memory (export "memory") 1)
          (func (export "run") (result i32)
            i32.const 0))
    "#;

    const POOLED_STATE_ISOLATION_WAT: &str = r#"
        (module
          (global $state (mut i32) (i32.const 7))
          (memory 1)
          (table $dispatch 1 funcref)
          (func $seed (result i32) (i32.const 0))
          (elem (i32.const 0) $seed)

          (func (export "mutate") (result i32)
            (global.set $state (i32.const 42))
            (i32.store8 (i32.const 0) (i32.const 99))
            (table.set $dispatch (i32.const 0) (ref.null func))
            (i32.const 0))

          (func (export "state_is_dirty") (result i32)
            (i32.or
              (i32.eq (global.get $state) (i32.const 42))
              (i32.or
                (i32.eq (i32.load8_u (i32.const 0)) (i32.const 99))
                (ref.is_null (table.get $dispatch (i32.const 0)))))))
    "#;

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

    fn test_pool() -> Arc<InstancePool> {
        InstancePool::new(PoolConfig {
            max_per_plugin: 1,
            max_total: 1,
            idle_timeout: Duration::from_secs(60),
            acquire_timeout: Duration::from_millis(50),
            call_timeout_ms: 5_000,
            call_memory_mb: 128,
            call_fuel: 10_000_000,
        })
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn assert_memory_limit_evidence(error: &str) {
        let normalized = error.to_ascii_lowercase();
        let explicit_oom = normalized == "oom" || normalized.contains("out of memory");
        let memory_boundary = normalized.contains("memory")
            && (normalized.contains("out of bounds")
                || normalized.contains("out-of-bounds")
                || normalized.contains("limit")
                || normalized.contains("maximum")
                || normalized.contains("grow"));
        assert!(
            explicit_oom || memory_boundary,
            "trap must contain OOM or linear-memory boundary evidence: {error}"
        );
        assert!(
            !normalized.contains("unreachable"),
            "probe must not pass through an arbitrary `unreachable` trap: {error}"
        );
    }

    #[test]
    fn memory_limit_megabytes_convert_to_wasm_pages() {
        assert_eq!(memory_limit_pages(1).expect("one MiB is valid"), 16);
        assert_eq!(memory_limit_pages(128).expect("128 MiB is valid"), 2048);
        assert!(memory_limit_pages(0).is_err());
        assert!(memory_limit_pages(u64::MAX).is_err());
    }

    #[tokio::test]
    async fn capability_only_plugin_runs_registered_host_call_without_wasi() {
        let bytes = wat::parse_str(HOST_CALL_ROUNDTRIP_WAT).expect("WAT must compile");
        let mut plugin =
            build_extism_plugin(bytes, 5_000, 128, 10_000_000).expect("plugin must instantiate");
        let db_pool = MySqlPoolOptions::new().connect_lazy_with(MySqlConnectOptions::new());
        let host_context = Arc::new(HostInvocationCtx {
            deps: DispatcherDeps {
                pool: db_pool,
                s3: None,
                registry: Arc::new(CapabilityRegistry::new()),
                llm: Arc::new(LlmRegistry::new()),
                rate_limits: Arc::new(CapabilityRateLimits::new()),
            },
            dispatch: DispatchCtx {
                execution_context: RuntimeExecutionContext::best_effort(
                    Some("host-call-no-wasi-test".into()),
                    None,
                ),
                agent_id: 3,
                plugin_id: 7,
                function_id: Some(5),
                permissions: vec![TIME_NOW.into()],
            },
        });
        let envelope = serde_json::json!({
            "capability": TIME_NOW,
            "args": {}
        })
        .to_string();

        let reply = tokio::task::spawn_blocking(move || {
            plugin.call_with_host_context::<&str, String, _>("run", envelope.as_str(), host_context)
        })
        .await
        .expect("Plugin task must join")
        .expect("registered host_call must remain available");
        let reply: serde_json::Value =
            serde_json::from_str(&reply).expect("host_call must return its JSON envelope");
        assert_eq!(reply["ok"], true);
        assert!(reply["data"]["unix_ms"].is_number());
    }

    #[test]
    fn shared_smoke_fixture_exports_the_v1_version_through_hiveweb() {
        let mut plugin = build_extism_plugin(SHARED_SMOKE_WASM.to_vec(), 5_000, 128, 10_000_000)
            .expect("shared fixture must instantiate in HiveWeb");

        let version = plugin
            .call::<_, String>(hive_runtime_core::wasm::ABI_VERSION_EXPORT, "{}")
            .expect("HiveWeb must be able to query the shared fixture ABI version");
        assert_eq!(version, hive_runtime_core::abi::HIVE_EXTISM_ABI_V1);
    }

    #[test]
    fn unsupported_abi_version_is_rejected_before_the_business_export() {
        let mut plugin = build_extism_plugin(
            shared_smoke_with_unsupported_abi_version(),
            5_000,
            128,
            10_000_000,
        )
        .expect("a structurally complete v2 fixture must instantiate before runtime ABI checking");
        let business_called = AtomicBool::new(false);

        let error = call_after_abi_check(&mut plugin, |_plugin| {
            business_called.store(true, Ordering::SeqCst);
            Ok::<(), extism::Error>(())
        })
        .expect_err("hive-extism/v2 must be rejected before the business export");

        assert!(
            error.to_string().contains("UnsupportedAbiVersion"),
            "runtime rejection must preserve the shared category: {error}"
        );
        assert!(
            !business_called.load(Ordering::SeqCst),
            "the business export must not run after an unsupported ABI version"
        );
    }

    #[test]
    fn abi_probe_call_failure_maps_to_unsupported_before_the_business_export() {
        let bytes = wat::parse_str(
            r#"(module
                (func (export "run") (result i32)
                  i32.const 0))"#,
        )
        .expect("WAT must compile");
        let mut plugin = build_extism_plugin(bytes, 5_000, 128, 10_000_000)
            .expect("the unchecked probe module must instantiate");
        let business_called = AtomicBool::new(false);

        let error = call_after_abi_check(&mut plugin, |_plugin| {
            business_called.store(true, Ordering::SeqCst);
            Ok::<(), extism::Error>(())
        })
        .expect_err("a missing ABI function must be mapped to the shared runtime category");

        assert_eq!(
            error.to_string(),
            "UnsupportedAbiVersion: wasm validation rejected: unsupported abi version"
        );
        assert!(
            !business_called.load(Ordering::SeqCst),
            "the business export must not run after an ABI probe failure"
        );
    }

    #[test]
    fn abi_probe_invalid_string_maps_to_unsupported_before_the_business_export() {
        let bytes = wat::parse_str(
            r#"(module
                (import "extism:host/env" "alloc"
                  (func $alloc (param i64) (result i64)))
                (import "extism:host/env" "store_u8"
                  (func $store_u8 (param i64 i32)))
                (import "extism:host/env" "output_set"
                  (func $output_set (param i64 i64)))
                (func (export "_hive_plugin_abi_version") (result i32)
                  (local $ptr i64)
                  (local.set $ptr (call $alloc (i64.const 1)))
                  (call $store_u8 (local.get $ptr) (i32.const 255))
                  (call $output_set (local.get $ptr) (i64.const 1))
                  i32.const 0))"#,
        )
        .expect("WAT must compile");
        let mut plugin = build_extism_plugin(bytes, 5_000, 128, 10_000_000)
            .expect("the invalid-string probe module must instantiate");
        let business_called = AtomicBool::new(false);

        let error = call_after_abi_check(&mut plugin, |_plugin| {
            business_called.store(true, Ordering::SeqCst);
            Ok::<(), extism::Error>(())
        })
        .expect_err("a non-UTF-8 ABI reply must map to the shared runtime category");

        assert_eq!(
            error.to_string(),
            "UnsupportedAbiVersion: wasm validation rejected: unsupported abi version"
        );
        assert!(
            !business_called.load(Ordering::SeqCst),
            "the business export must not run after ABI reply decoding fails"
        );
    }

    #[test]
    fn production_builder_revalidates_fetched_bytes_before_compilation() {
        let bytes = wat::parse_str(HOST_CALL_ROUNDTRIP_WAT).expect("WAT must compile");
        build_extism_compiled_plugin(bytes.clone(), 5_000, 128, 10_000_000)
            .expect("the low-level resource probe builder intentionally skips ABI validation");

        let error = build_validated_extism_compiled_plugin(bytes, 5_000, 128, 10_000_000)
            .err()
            .expect("the production builder must reject a missing ABI export");
        assert!(
            error.to_string().contains("MissingAbiVersionExport"),
            "production runtime validation must preserve the shared category: {error}"
        );

        build_validated_extism_compiled_plugin(SHARED_SMOKE_WASM.to_vec(), 5_000, 128, 10_000_000)
            .expect("the production builder must accept the shared v1 fixture");
    }

    #[tokio::test]
    async fn shared_smoke_fixture_has_the_exact_hiveweb_echo_reply() {
        let mut plugin = build_extism_plugin(SHARED_SMOKE_WASM.to_vec(), 5_000, 128, 10_000_000)
            .expect("shared fixture must instantiate in HiveWeb");
        let db_pool = MySqlPoolOptions::new().connect_lazy_with(MySqlConnectOptions::new());
        let host_context = Arc::new(HostInvocationCtx {
            deps: DispatcherDeps {
                pool: db_pool,
                s3: None,
                registry: Arc::new(CapabilityRegistry::new()),
                llm: Arc::new(LlmRegistry::new()),
                rate_limits: Arc::new(CapabilityRateLimits::new()),
            },
            dispatch: DispatchCtx {
                execution_context: RuntimeExecutionContext::best_effort(
                    Some("shared-smoke-hiveweb-test".into()),
                    None,
                ),
                agent_id: 3,
                plugin_id: 7,
                function_id: Some(5),
                permissions: vec![LOG_EMIT.into()],
            },
        });

        let reply = tokio::task::spawn_blocking(move || {
            call_after_abi_check(&mut plugin, |plugin| {
                plugin.call_with_host_context::<&str, String, _>("echo", r#""hello""#, host_context)
            })
        })
        .await
        .expect("Plugin task must join")
        .expect("the shared smoke fixture must execute through HiveWeb");

        assert_eq!(reply, r#"{"echo":"hello","logged":true}"#);
    }

    #[test]
    fn capability_only_runtime_rejects_wasi_imports() {
        let bytes = wat::parse_str(WASI_IMPORT_WAT).expect("WAT must compile");
        let result = build_extism_plugin(bytes, 5_000, 128, 10_000_000);
        assert!(
            result.is_err(),
            "WASI imports must not be linked into a capability-only Plugin"
        );
    }

    #[tokio::test]
    async fn plugin_lookup_failure_emits_exactly_one_safe_audit() {
        let writer = TraceWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer.clone())
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let db_pool = MySqlPoolOptions::new()
            .acquire_timeout(Duration::from_millis(50))
            .connect_lazy_with(
                MySqlConnectOptions::new()
                    .host("127.0.0.1")
                    .port(1)
                    .username("PLUGIN_LOOKUP_DB_SENTINEL"),
            );
        let invoker = Invoker::new(test_pool());
        let result = invoker
            .invoke(
                &db_pool,
                None,
                Arc::new(CapabilityRegistry::new()),
                Arc::new(LlmRegistry::new()),
                7,
                "run",
                r#"{"private":"PLUGIN_INPUT_SENTINEL"}"#.into(),
                DispatchCtx {
                    execution_context: RuntimeExecutionContext::best_effort(
                        Some("plugin-lookup-audit-test".into()),
                        Some(11),
                    ),
                    agent_id: 3,
                    plugin_id: 7,
                    function_id: Some(5),
                    permissions: vec![TIME_NOW.into()],
                },
            )
            .await;
        assert!(result.is_err(), "offline plugin lookup must fail");

        let output =
            String::from_utf8(writer.0.lock().expect("trace buffer poisoned").clone()).unwrap();
        assert_eq!(
            output.matches("event_type=\"plugin_invoke\"").count(),
            1,
            "early Invoker failures must emit exactly one audit: {output}"
        );
        assert!(output.contains("outcome=\"error\""));
        assert!(output.contains("error_kind=\"plugin_invocation_failed\""));
        for sentinel in ["PLUGIN_LOOKUP_DB_SENTINEL", "PLUGIN_INPUT_SENTINEL"] {
            assert!(
                !output.contains(sentinel),
                "Invoker audit leaked {sentinel}: {output}"
            );
        }
    }

    #[test]
    fn plugin_audit_guard_finalizes_exactly_once_and_uses_static_errors() {
        let writer = TraceWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer.clone())
            .finish();
        let _subscriber_guard = tracing::subscriber::set_default(subscriber);
        {
            let mut audit = PluginInvokeAuditGuard::new(
                RuntimeExecutionContext::best_effort(
                    Some("plugin-finalize-audit-test".into()),
                    Some(11),
                ),
                3,
                7,
                Some(5),
                Instant::now(),
            );
            audit.finish("error", true);
            audit.finish("success", false);
        }

        let output =
            String::from_utf8(writer.0.lock().expect("trace buffer poisoned").clone()).unwrap();
        assert_eq!(
            output.matches("event_type=\"plugin_invoke\"").count(),
            1,
            "explicit finalize plus Drop must emit exactly one audit: {output}"
        );
        assert!(output.contains("outcome=\"error\""));
        assert!(output.contains("error_kind=\"plugin_invocation_failed\""));
        assert!(output.contains("error_message=\"Plugin invocation failed\""));
    }

    #[test]
    fn successful_call_with_failed_reset_returns_output_but_audits_error_once() {
        let writer = TraceWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer.clone())
            .finish();
        let _subscriber_guard = tracing::subscriber::set_default(subscriber);
        let result = {
            let mut audit = PluginInvokeAuditGuard::new(
                RuntimeExecutionContext::best_effort(
                    Some("plugin-reset-audit-test".into()),
                    Some(11),
                ),
                3,
                7,
                Some(5),
                Instant::now(),
            );
            finalize_plugin_invocation(
                &mut audit,
                Some(r#"{"ok":true}"#.into()),
                "success",
                None,
                false,
                5_000,
            )
        };

        assert_eq!(
            result.expect("reset failure must not replace successful Plugin output"),
            r#"{"ok":true}"#
        );
        let output =
            String::from_utf8(writer.0.lock().expect("trace buffer poisoned").clone()).unwrap();
        assert_eq!(
            output.matches("event_type=\"plugin_invoke\"").count(),
            1,
            "reset failure must finalize exactly one Plugin audit: {output}"
        );
        assert!(output.contains("outcome=\"error\""));
        assert!(output.contains("error_kind=\"plugin_invocation_failed\""));
        assert!(!output.contains("outcome=\"success\""));
    }

    #[tokio::test]
    async fn pooled_compiled_plugin_creates_fresh_global_memory_and_table_state_per_checkout() {
        let pool = test_pool();
        let bytes = wat::parse_str(POOLED_STATE_ISOLATION_WAT).expect("WAT must compile");
        let row = PluginRow {
            id: 7,
            identifier: "state-isolation".into(),
            name: "State isolation".into(),
            description: None,
            manifest: None,
            runtime: "extism".into(),
            version: "1.0.0".into(),
            author: None,
            repository_url: None,
            s3_key: "state-isolation.wasm".into(),
            sha256: sha256_hex(&bytes),
            size_bytes: bytes.len() as i64,
            category_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
        };

        let mut first = pool
            .acquire_with_loader(
                &row,
                move || async move { Ok(bytes) },
                |bytes| build_extism_compiled_plugin(bytes, 5_000, 128, 10_000_000),
            )
            .await
            .expect("first checkout must compile the prepared slot");
        first
            .plugin
            .call::<_, String>("mutate", "")
            .expect("state mutation must succeed");
        pool.release(first, true, true).await;

        let mut second = pool
            .acquire_with_loader(
                &row,
                || async {
                    Err(PoolError::S3(
                        "compiled-slot hit must not reload stateful Wasm".into(),
                    ))
                },
                |bytes| build_extism_compiled_plugin(bytes, 5_000, 128, 10_000_000),
            )
            .await
            .expect("second checkout must reuse only the compiled artifact");
        second
            .plugin
            .call::<_, String>("state_is_dirty", "")
            .expect("each checkout must create a fresh Store and Instance");
        pool.release(second, true, true).await;
    }

    #[tokio::test]
    async fn aborting_real_wasm_invocation_releases_slot_without_repooling_or_double_release() {
        let writer = TraceWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer.clone())
            .finish();
        let _subscriber_guard = tracing::subscriber::set_default(subscriber);

        let pool = InstancePool::new(PoolConfig {
            max_per_plugin: 1,
            max_total: 1,
            idle_timeout: Duration::from_secs(60),
            acquire_timeout: Duration::from_secs(5),
            call_timeout_ms: 5_000,
            call_memory_mb: 128,
            call_fuel: 10_000_000_000,
        });
        let bytes = wat::parse_str(CPU_LOOP_WAT).expect("WAT must compile");
        let invoker = Arc::new(Invoker::new(Arc::clone(&pool)));
        let db_pool = MySqlPoolOptions::new().connect_lazy_with(MySqlConnectOptions::new());
        let row = PluginRow {
            id: 7,
            identifier: "abort-real-wasm".into(),
            name: "Abort real WASM".into(),
            description: None,
            manifest: None,
            runtime: "extism".into(),
            version: "1.0.0".into(),
            author: None,
            repository_url: None,
            s3_key: "abort-real-wasm.wasm".into(),
            sha256: sha256_hex(&bytes),
            size_bytes: bytes.len() as i64,
            category_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
        };
        let prepared = pool
            .acquire_with_loader(
                &row,
                move || async move { Ok(bytes) },
                |bytes| build_extism_compiled_plugin(bytes, 5_000, 128, 10_000_000_000),
            )
            .await
            .expect("real CPU-loop Plugin template must compile");
        pool.release(prepared, true, true).await;
        let invocation = tokio::spawn({
            let invoker = Arc::clone(&invoker);
            let db_pool = db_pool.clone();
            async move {
                let mut audit = PluginInvokeAuditGuard::new(
                    RuntimeExecutionContext::best_effort(
                        Some("plugin-abort-audit-test".into()),
                        Some(11),
                    ),
                    3,
                    7,
                    Some(5),
                    Instant::now(),
                );
                invoker
                    .invoke_resolved(
                        &db_pool,
                        None,
                        Arc::new(CapabilityRegistry::new()),
                        Arc::new(LlmRegistry::new()),
                        row,
                        "spin",
                        r#"{"private":"ABORT_INPUT_SENTINEL"}"#.into(),
                        DispatchCtx {
                            execution_context: RuntimeExecutionContext::best_effort(
                                Some("plugin-abort-audit-test".into()),
                                Some(11),
                            ),
                            agent_id: 3,
                            plugin_id: 7,
                            function_id: Some(5),
                            permissions: vec![TIME_NOW.into()],
                        },
                        &mut audit,
                    )
                    .await
            }
        });

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let metrics = pool.metrics_snapshot().await;
                if metrics.in_use == 1 && metrics.idle == 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("real WASM invocation must acquire the only pool slot");
        tokio::time::sleep(Duration::from_millis(25)).await;
        invocation.abort();
        assert!(
            invocation
                .await
                .expect_err("aborted invocation must not return")
                .is_cancelled(),
            "JoinError must report caller cancellation"
        );

        let released = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let metrics = pool.metrics_snapshot().await;
                if metrics.in_use == 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_ok();
        if !released {
            pool.discard_lost(7).await;
        }
        assert!(
            released,
            "aborting the Invoker future must release its checked-out pool slot"
        );

        tokio::time::sleep(Duration::from_millis(50)).await;
        let metrics = pool.metrics_snapshot().await;
        let per_plugin = pool.per_plugin_snapshot().await;
        assert_eq!(metrics.in_use, 0);
        assert_eq!(
            metrics.idle, 0,
            "an aborted instance must never be re-pooled"
        );
        assert_eq!(per_plugin[0].in_use, 0, "slot must not be released twice");
        assert_eq!(per_plugin[0].idle, 0);

        let output =
            String::from_utf8(writer.0.lock().expect("trace buffer poisoned").clone()).unwrap();
        assert_eq!(
            output.matches("event_type=\"plugin_invoke\"").count(),
            1,
            "caller abort must emit exactly one safe Plugin audit: {output}"
        );
        assert!(output.contains("outcome=\"error\""));
        assert!(!output.contains("ABORT_INPUT_SENTINEL"));
    }

    #[test]
    fn plugin_memory_probe_succeeds_at_128_mib_and_traps_above_the_limit() {
        let bytes = wat::parse_str(MEMORY_LIMIT_PROBE_WAT).expect("WAT must compile");
        let mut within_limit = build_extism_plugin(bytes.clone(), 5_000, 128, 10_000_000)
            .expect("plugin should instantiate");
        let output = within_limit
            .call::<&str, String>("allocate_within_limit", "{}")
            .expect("allocation and write at the 128 MiB boundary must succeed");
        assert!(output.is_empty(), "probe returns no output payload");

        let mut over_limit =
            build_extism_plugin(bytes, 5_000, 128, 10_000_000).expect("plugin should instantiate");
        let error = over_limit
            .call::<&str, String>("allocate_over_limit", "{}")
            .expect_err("129 MiB allocation must exceed the 128 MiB limit");
        let raw_error = error.to_string();
        assert_memory_limit_evidence(&raw_error);

        let app_error: AppError = InvokerError::PluginError(error.to_string()).into();

        assert_eq!(app_error.code(), codes::INTERNAL);
        assert_eq!(app_error.message(), "Plugin 执行失败或超过内存上限");
        assert!(
            !app_error.message().contains(&raw_error),
            "external error must not expose the runtime trap"
        );
        assert_ne!(
            app_error.code(),
            codes::PLUGIN_INVOCATION_TIMEOUT,
            "memory-limit failures must not be reported as timeouts"
        );
    }

    #[test]
    fn cpu_loop_exhausts_fuel_and_maps_to_timeout() {
        let bytes = wat::parse_str(CPU_LOOP_WAT).expect("WAT must compile");
        let mut plugin =
            build_extism_plugin(bytes, 5_000, 128, 10_000).expect("plugin should instantiate");

        let error = plugin
            .call::<&str, String>("spin", "{}")
            .expect_err("an infinite CPU loop must exhaust its fuel budget");
        let raw_error = error.to_string();
        assert!(
            raw_error.to_ascii_lowercase().contains("fuel"),
            "CPU loop must be interrupted by fuel, not an unrelated trap: {raw_error}"
        );
        assert!(plugin_error_is_timeout(&raw_error));

        let app_error: AppError = InvokerError::Timeout(30_000).into();
        assert_eq!(app_error.code(), codes::PLUGIN_INVOCATION_TIMEOUT);
    }

    #[tokio::test]
    async fn outer_timeout_cancels_a_real_wasm_cpu_loop_near_the_budget() {
        const OUTER_BUDGET_MS: u64 = 50;

        let bytes = wat::parse_str(CPU_LOOP_WAT).expect("WAT must compile");
        // Keep the internal Extism deadline well beyond the outer budget and
        // give the loop enough fuel that only the explicit cancel should stop
        // it during this test window.
        let mut plugin = build_extism_plugin(bytes, 5_000, 128, 10_000_000_000)
            .expect("plugin should instantiate");
        let cancel_handle = plugin.cancel_handle();

        let pool = InstancePool::new(PoolConfig {
            max_per_plugin: 1,
            max_total: 1,
            idle_timeout: Duration::from_secs(60),
            acquire_timeout: Duration::from_secs(5),
            call_timeout_ms: OUTER_BUDGET_MS,
            call_memory_mb: 128,
            call_fuel: 10_000_000_000,
        });
        pool.seed_in_use_for_test(43, "real-cancel-plugin", "1.0.0");

        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let task = tokio::task::spawn_blocking(move || {
            let _ = started_tx.send(());
            let started_at = Instant::now();
            let result = plugin
                .call::<&str, String>("spin", "{}")
                .map_err(|error| error.to_string());
            let _ = finished_tx.send((started_at.elapsed(), result));
        });
        started_rx
            .await
            .expect("blocking task must start before its outer budget is measured");

        let cancel_called = Arc::new(AtomicBool::new(false));
        let cancel_observation = Arc::clone(&cancel_called);
        let outer_started_at = Instant::now();
        let result = await_blocking_call(&pool, 43, OUTER_BUDGET_MS, task, move || {
            cancel_observation.store(true, Ordering::SeqCst);
            cancel_handle.cancel()
        })
        .await;
        let outer_elapsed = outer_started_at.elapsed();

        assert!(matches!(result, Err(BlockingCallError::WallClockTimeout)));
        assert!(
            cancel_called.load(Ordering::SeqCst),
            "outer timeout must explicitly signal the Extism cancel handle"
        );
        let app_error: AppError = InvokerError::Timeout(OUTER_BUDGET_MS).into();
        assert_eq!(app_error.code(), codes::PLUGIN_INVOCATION_TIMEOUT);
        assert!(
            outer_elapsed >= Duration::from_millis(OUTER_BUDGET_MS),
            "outer timeout returned before its budget: {outer_elapsed:?}"
        );
        assert!(
            outer_elapsed < Duration::from_millis(750),
            "outer timeout should return near its budget: {outer_elapsed:?}"
        );

        let (wasm_elapsed, wasm_result) = tokio::time::timeout(Duration::from_secs(1), finished_rx)
            .await
            .expect("Extism cancellation must stop the Wasm CPU loop promptly")
            .expect("blocking task must report its completion");
        let raw_error = wasm_result.expect_err("cancelled infinite loop must trap");
        assert!(
            plugin_error_is_timeout(&raw_error),
            "cancelled CPU loop must report timeout/cancellation evidence: {raw_error}"
        );
        assert!(
            wasm_elapsed < Duration::from_secs(1),
            "cancelled Wasm should finish promptly: {wasm_elapsed:?}"
        );

        let metrics = pool.metrics_snapshot().await;
        let per_plugin = pool.per_plugin_snapshot().await;
        assert_eq!(metrics.in_use, 0);
        assert_eq!(metrics.idle, 0);
        assert_eq!(per_plugin[0].in_use, 0);
        assert_eq!(per_plugin[0].idle, 0);
    }

    #[tokio::test]
    async fn wall_clock_timeout_discards_pool_slot_once_while_task_finishes_detached() {
        let pool = InstancePool::new(PoolConfig {
            max_per_plugin: 1,
            max_total: 1,
            idle_timeout: Duration::from_secs(60),
            acquire_timeout: Duration::from_secs(5),
            call_timeout_ms: 10,
            call_memory_mb: 128,
            call_fuel: 10_000,
        });
        pool.seed_in_use_for_test(42, "wall-timeout-plugin", "1.0.0");

        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let task_finished = Arc::clone(&finished);
        let cancel_called = Arc::new(AtomicBool::new(false));
        let cancel_observation = Arc::clone(&cancel_called);
        let task = tokio::task::spawn_blocking(move || {
            release_rx
                .recv()
                .expect("test should release the detached blocking task");
            task_finished.store(true, Ordering::SeqCst);
            7
        });

        let result = await_blocking_call(&pool, 42, 10, task, move || {
            cancel_observation.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await;
        assert!(matches!(result, Err(BlockingCallError::WallClockTimeout)));
        assert!(
            cancel_called.load(Ordering::SeqCst),
            "outer timeout must invoke cancellation before returning"
        );
        let metrics = pool.metrics_snapshot().await;
        let per_plugin = pool.per_plugin_snapshot().await;
        assert_eq!(metrics.in_use, 0, "timeout must free the slot immediately");
        assert_eq!(metrics.idle, 0, "timed-out instances must never be pooled");
        assert_eq!(per_plugin[0].in_use, 0);
        assert_eq!(per_plugin[0].idle, 0);

        release_tx
            .send(())
            .expect("detached blocking task should still be alive");
        tokio::time::timeout(Duration::from_secs(1), async {
            while !finished.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("detached blocking task should eventually finish and drop its value");

        let metrics = pool.metrics_snapshot().await;
        let per_plugin = pool.per_plugin_snapshot().await;
        assert_eq!(metrics.in_use, 0, "task completion must not release twice");
        assert_eq!(
            metrics.idle, 0,
            "task completion must not re-pool the value"
        );
        assert_eq!(per_plugin[0].in_use, 0);
        assert_eq!(per_plugin[0].idle, 0);
    }
}
