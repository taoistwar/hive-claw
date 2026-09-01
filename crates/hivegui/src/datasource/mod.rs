//! Data source public boundary for feature 011.
//!
//! This module re-exports concrete store/runtime helpers used by higher-level UI
//! and runtime orchestration.
//!
//! 变更公开 API 清单（feature 011）：完整导出清单与复杂度/安全性核对见
//! `specs/011-hivegui-standalone-mode/checklists/changed-public-api.md`。
//!
//! 安全约束：
//! - 按 feature 规范，公开 SQL 与查询策略依赖下游 `query_plan` 与 `sql_source_inventory`。
//! - `function_not_executable` 与其他稳定错误语义由 ABI/diagnostics 边界统一映射。
//! - 不在本入口直接引入未标注所有者的复杂副作用。

pub mod backup;
pub mod capability_store;
pub mod category_store;
pub mod conversation_store;
pub mod crypto;
pub mod data_source_store;
pub mod entity_store;
pub mod function_store;
pub mod global_config_store;
pub mod key_store;
pub mod llm_provider_store;
pub mod llm_store;
pub mod migrations;
pub mod models;
pub mod mysql_client;
pub mod plugin_artifacts;
pub mod plugin_manifest;
pub mod query_count;
pub mod query_plan;
pub mod search_index;
pub mod search_normalization;
pub mod skill_store;
pub mod sql_source_inventory;
pub mod store;
pub mod tag_store;
pub mod tool_store;
pub mod validation;
pub mod wasm_exports;
pub mod workflow_store;

pub use capability_store::{
    CapabilityConflict, CapabilityFilter, CapabilityInput, CapabilityPage, CapabilityRecord,
    CapabilityStore, CapabilityStoreError, CapabilityStoreErrorKind,
};
pub use category_store::{
    CategoryConflict, CategoryInput, CategoryNode, CategoryStore, CategoryStoreError,
    CategoryStoreErrorKind, CategoryTree, CycleError, DeletePlan, ReferenceKind,
};
pub use crypto::Crypto;
pub use data_source_store::{
    Conflict, ConflictReason, CreateCancel, DataSourceFilter, DataSourceInput, DataSourcePage,
    DataSourceRecord, DataSourceStore, DataSourceStoreError, DataSourceStoreErrorKind,
    DataSourceViewMode, EmptyPasswordPolicy,
};
pub use function_store::{
    FunctionInput, FunctionKind, FunctionPage, FunctionRecord, FunctionStore,
    RESERVED_UNDERSCORE_IDENTIFIERS,
};
pub use global_config_store::{
    GlobalConfigConflict, GlobalConfigFilter, GlobalConfigInput, GlobalConfigPage,
    GlobalConfigRecord, GlobalConfigStore, GlobalConfigStoreError, GlobalConfigStoreErrorKind,
};
pub use llm_provider_store::{
    LlmProviderConflict, LlmProviderInput, LlmProviderRecord, LlmProviderStore,
    LlmProviderStoreError, LlmProviderStoreErrorKind, LlmProviderTokenInput, MaskedToken,
};
pub use models::*;
pub use mysql_client::MysqlClient;
pub use skill_store::{SkillInput, SkillRecord, SkillStore, SkillStoreError, SkillStoreErrorKind};
pub use store::{
    CorruptionDecision, CorruptionRecoveryReport, CorruptionReport, DatabaseFailureClass,
    GlobalConfig, OpenOutcome, QuarantineReason, QuarantineRecord, RetryPolicy, RetrySleeper,
    SidecarKind, SidecarRecord, Store, StoreError, StoreErrorKind, StoreOpenError,
    StoreOpenErrorKind, StoreOpenFaultInjector, StoreOpenOptions, StoreStartupGate, WriteGate,
};
pub use tag_store::{
    TagConflict, TagFilter, TagInput, TagPage, TagRecord, TagStore, TagStoreError,
    TagStoreErrorKind,
};
pub use tool_store::{
    ToolInput, ToolKind, ToolPage, ToolRecord, ToolSource, ToolStore, ToolStoreError,
    ToolStoreErrorKind,
};
pub use workflow_store::{
    NodeType, WorkflowConflict, WorkflowEdge, WorkflowGraph, WorkflowGraphBuilder, WorkflowNode,
    WorkflowPage, WorkflowRecord, WorkflowStore, WorkflowStoreError,
};
