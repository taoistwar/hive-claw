//! Storage- and transport-independent Agent runtime contracts.
//!
//! This crate is the shared protocol and pure-algorithm boundary used by both
//! HiveWeb and HiveGUI. It owns no database, filesystem, network client, UI, or
//! product-specific lifecycle. Callers provide those concerns through adapters.
//! In particular, sharing this crate never permits HiveGUI to request or fall
//! back to HiveWeb.
//!
//! 变更公开 API 清单（feature 011）：本文件导出边界与不变量说明记录在
//! `specs/011-hivegui-standalone-mode/checklists/changed-public-api.md`。
//!
//! 安全约束：仅保留 ABI、执行、能力、WASM、workflow、持久化工具的稳定公共边界；
//! 不在此层引入数据库、文件系统、网络客户端或 UI 副作用。
//! `StableErrorKind::FunctionNotExecutable` 对应稳定错误名 `function_not_executable`。
//!
//! The modules intentionally begin as documented public seams. Versioned ABI
//! validation, execution cancellation, capability dispatch, plugin isolation,
//! WASM validation, workflow scheduling, and persisted Tool DTOs are implemented
//! in their dedicated feature tasks without adding product storage or transport
//! dependencies here.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod abi;
pub mod capability;
pub mod execution;
pub mod persisted_tool;
pub mod plugin;
pub mod wasm;
pub mod workflow;
