# Feature 011 Changed Public API Inventory

## Scope and evidence

- Feature: `011-hivegui-standalone-mode`
- Modules covered by this task: `hive-runtime-core/src/lib.rs`, `hivegui/src/runtime/mod.rs`, `hivegui/src/datasource/mod.rs`
- Baseline date: `2026-08-13`
- Validation: static source audit against module declarations + manual enum/error invariant mapping review.

## 1. `hive-runtime-core/src/lib.rs`

### Export surface introduced/kept

- `abi` module and items it re-exports via downstream module usage:
  - `StableErrorKind`（含 `FunctionNotExecutable`，与稳定错误名 `function_not_executable` 形成一一约束）
  - `ExecutionContext`/`ExecutionRequest`/`ExecutionResponse`（执行边界类型）
  - `PluginManifestV1`、`WasmModuleShape`、`WasmSandboxConfig`、`WasmExecutionFailure`
- `capability` module items (capability 集合与匹配规则类型)
- `execution` module items (上下文与执行失败边界类型)
- `persisted_tool` module items (持久化 tool 快照 DTO)
- `plugin` module items (能力/白名单边界)
- `workflow` module items (`NodeType`, `WorkflowValidationKind`, 拓扑/结果边界类型)

### 输入/输出与稳定性说明

- `crate` 为存储与传输无关边界：禁止新增数据库/HTTP/网络依赖；对外 API 维持稳定结构性类型。
- 所有 `pub` 符号保留为模块边界公开契约，不直接承接应用层副作用。
- 与 `StableErrorKind::FunctionNotExecutable` 对齐的稳定错误语义定义固定在 ABI 文档层：
  - 不可执行路径返回统一的稳定错误语义，不携带可注入 SQL 或明文业务上下文。

## 2. `hivegui/src/runtime/mod.rs`

### Export surface introduced/kept

- `pub mod`：`builtin_executor`, `capability_adapter`, `desktop_host`, `diagnostics`, `execution`, `function_test_executor`, `plugin_executor`, `provider_resolver`, `tool_adapter`
- `pub use` re-export:
  - Execution：`CANCEL_DEADLINE`, `CancelHandle`, `ExecutionId`, `FailureCategory`, `FoundationRuntimeComposition`, `LocalExecutionAdapter`, `LocalExecutionError`, `LocalExecutionFuture`, `LocalExecutionOutcome`, `LocalExecutionRequest`, `LocalFunctionExecutionAdapter`
  - Tool/Adapter：`AdapterErrorCode`, `AuditSink`, `CapabilityAdapter`, `CapabilityDispatchError`, `CapabilityHandler`, `DispatchAuditEvent`, `DispatchOutcome`, `HandlerRegistry`, `NullAuditSink`
  - Desktop host：`DesktopHostDispatcher`
  - Plugin/Function test：`PluginExecutor`, `FUNCTION_TEST_TIMEOUT`, `FunctionTestExecutor`, `capability_matches_filter`, `format_test_output`, `resolve_test_capabilities`
  - Provider/Transport：`ProviderAttempt`, `ProviderCallOutcome`, `ProviderCallRequest`, `ProviderError`, `ProviderErrorKind`, `ProviderResolver`, `ProviderTransport`, `TransportError`, `TransportOutcome`, `TransportRequest`

### 安全不变量

- 执行层仅走本地 composition 与本地 adapter，不构造远端 HiveWeb 入口（Feature boundary: “HiveGUI 不请求 HiveWeb”）。
- 取消与失败路径必须保持可恢复的 `FailureCategory` 一致性：
  - `FailureCategory::Internal`、`FailureCategory::InvalidInput`、`FailureCategory::FunctionNotExecutable` 等稳定边界由执行器统一映射。
- 本层不引入运行时 SQL 或数据库副作用；跨模块依赖仅在下游 datasource/runtime 上。

## 3. `hivegui/src/datasource/mod.rs`

### Export surface introduced/kept

- `pub mod`：`backup`, `capability_store`, `category_store`, `conversation_store`, `crypto`, `data_source_store`, `entity_store`, `function_store`, `global_config_store`, `key_store`, `llm_provider_store`, `llm_store`, `migrations`, `models`, `mysql_client`, `plugin_artifacts`, `query_count`, `query_plan`, `search_index`, `search_normalization`, `skill_store`, `sql_source_inventory`, `store`, `tag_store`, `tool_store`, `validation`, `wasm_exports`, `workflow_store`
- `pub use` re-export（核心）:
  - Store/错误：`Crypto`, `GlobalConfig`, `Store`, `StoreOpenError`, `StoreOpenOptions`, `StoreStartupGate`, `WriteGate`, `RetryPolicy`, `RetrySleeper`, `CorruptionReport`, `QuarantineRecord`
  - 会话/数据结构：`DataSourceInput`, `DataSourceFilter`, `FunctionInput`, `WorkflowGraphBuilder`, `ToolKind`, `Search*`, `ConversationStore`, `MysqlClient` 等
  - 能力/关系：`CategoryTree`, `CapabilityConflict`, `ReferenceKind`, `NodeType`, `Conflict`/`ConflictReason`

### 安全不变量

- SQL 路径安全边界由下游 `query_plan`/`storage_query_plans` 的 inventory 覆盖，不在该入口直接引入不受控 `QueryBuilder`。
- `search_normalization` 仅暴露固定 `hivegui-nfkc-casefold-v1` 流程与版本化规范化约束；`function_not_executable` 与其他稳定错误仍保持 ABI / diagnostics 映射一致。
- `sql_source_inventory` 通过 ownership phase 标记与静态查询清单闭环，未将 future/stories API 暴露为运行时代码。

## Complexity and dead-code audit

- 上述 3 个入口文件本质为门面（入口转发表），当前仅含模块定义与 re-export。
- 公开 API 复杂度基线：仅为路由/再导出，不包含嵌套条件分支、循环、共享状态写入。
- 无 `TODO`、`todo!`、`unimplemented!`、`allow(dead_code)` 记录（对本文件级别范围）：`crates/hive-runtime-core/src/lib.rs`、`crates/hivegui/src/runtime/mod.rs`、`crates/hivegui/src/datasource/mod.rs`。

