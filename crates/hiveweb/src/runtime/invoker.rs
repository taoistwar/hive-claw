//! Plugin 调用入口（FR-006 / FR-029）
//!
//! 流程：resolve plugin（DB） → acquire from pool → invoke export → audit。
//! 当前为骨架；具体 invoke + reset 等接 Extism 后再补。

use crate::runtime::pool::InstancePool;
use std::sync::Arc;

#[derive(Debug)]
pub struct InvocationCtx {
    pub request_id: Option<String>,
    pub session_id: Option<i64>,
    pub agent_id: Option<i64>,
}

#[derive(Debug, thiserror::Error)]
pub enum InvokerError {
    #[error("plugin not found: {0}")]
    PluginMissing(i64),
    #[error("pool busy")]
    PoolBusy,
    #[error("timeout after {0}ms")]
    Timeout(u64),
    #[error("plugin runtime error: {0}")]
    PluginError(String),
}

#[derive(Debug)]
pub struct Invoker {
    pub pool: Arc<InstancePool>,
}

impl Invoker {
    pub fn new(pool: Arc<InstancePool>) -> Self {
        Self { pool }
    }

    /// Stub: 调用 Plugin 的导出函数。具体实现将：
    /// 1. 从 DB 取 plugin 元数据（sha256, s3_key）
    /// 2. 从 InstancePool acquire（命中或从 S3 拉 wasm + 校验 sha256 + 编译）
    /// 3. 双层 timeout (fuel + tokio) 调用 export
    /// 4. reset → 归还
    /// 5. 写 runtime_audit_logs
    #[allow(unused_variables)]
    pub async fn invoke(
        &self,
        _plugin_id: i64,
        _export: &str,
        _args: serde_json::Value,
        _ctx: &InvocationCtx,
    ) -> Result<serde_json::Value, InvokerError> {
        Err(InvokerError::PluginError(
            "invoker not yet implemented (Phase 3 US1)".into(),
        ))
    }
}
