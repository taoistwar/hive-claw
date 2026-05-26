//! Capability handlers — 每个 capability 一个 handler 文件 (T095..T101 / US4)。
//!
//! 入口在 `runtime::capability::dispatch`，由 Plugin 侧 `host_call(envelope)` 触发。
//! Handler 不直接做权限检查；调用前 dispatcher 已通过 Agent.permissions 过滤。

pub mod fs;
pub mod network_http;
pub mod s3;
pub mod secret;
pub mod utility;

// db.query / db.execute / llm.invoke 待 US5 阶段接 named_queries.toml +
// providers::FallbackProvider 后再补；dispatcher 当前返 5001 占位。
