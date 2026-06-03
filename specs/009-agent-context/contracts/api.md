# Contract: Agent Context API

## Overview

Agent Context 的 Rust API 契约，定义了核心 trait、struct 和方法签名。

**线程安全保证（FR-012, SC-005）**：
- `AgentContext` 实现 `Send + Sync`，支持跨线程共享
- 分类级别写入锁：不同 Category 拥有独立的 `Arc<RwLock<T>>`，不同类别可并发写入
- `ReadView` 实现 `Send + Sync`，支持跨线程只读访问
- `snapshot()` 获取所有分类读锁后创建深拷贝，快照创建期间写入操作被阻塞

**生命周期写入约束（FR-015, FR-017）**：
- `Active` 状态：所有写入操作（set_record, append_record, add_extension, set_response_payload, set_lifecycle_state）均允许
- `Completed` 状态：任何写入操作返回 `ContextError::IllegalStateTransition`
- `Terminated` 状态：任何写入操作返回 `ContextError::IllegalStateTransition`（除非 debug_mode 启用，允许 snapshot 创建）
- `Merging` 状态：仅 `merge_subagent_context` 操作允许，其他写入返回 `ContextError::IllegalStateTransition`

**集成约束（IL-001 ~ IL-004）**：
- IL-001：AgentContext 必须在 TurnContext 之前创建（调用方责任，非 API 强制）
- IL-002：AgentContextSyncHook 执行顺序在调用方 Hook 链中配置（非 API 强制）
- IL-003：AgentLoop 发生 panic/超时后，Context 的 `Arc` 引用计数归零即自动释放（Rust Drop 保证，非 API 强制）
- IL-004：debug_mode 通过 ContextConfig 控制（`ContextConfig::debug_mode: bool`）

---

## AgentContext

### Creation

```rust
impl AgentContext {
    /// 创建一个新的 Agent Context
    pub fn new(context_id: String, user_input: UserInput, config: ContextConfig) -> Self;

    /// 创建子 Agent Context（从主 Context 复制，应用黑名单）
    ///
    /// 约束：
    /// - blacklisted_categories 不得包含 Category::Entities 以外的所有类别（UserInput 始终对子 Agent 可见）
    /// - 若黑名单包含不允许的类别，返回 ContextError::InvalidBlacklist
    /// - 子 Context 继承主 Context 的 schema_version 和 audit_log 快照
    pub fn fork_for_subagent(
        &self,
        subagent_id: String,
        blacklisted_categories: Vec<Category>,
        config: ContextConfig,
    ) -> Result<Self, ContextError>;
}
```

### Read Operations

```rust
impl AgentContext {
    /// 获取用户输入（只读引用）
    pub fn user_input(&self) -> &UserInput;

    /// 获取某类别下的所有记录（只读视图）
    pub fn get_category(&self, category: Category) -> Result<Vec<RecordEntry>, ContextError>;

    /// 获取某类别下指定 key 的记录
    pub fn get_record(&self, category: Category, key: &str) -> Result<Option<RecordEntry>, ContextError>;

    /// 查询所有扩展内容
    pub fn get_extensions(&self) -> Vec<ExtensionContent>;

    /// 获取生命周期状态
    pub fn lifecycle_state(&self) -> LifecycleState;

    /// 获取最终响应负载（只读引用）
    pub fn get_response_payload(&self) -> Option<&ResponsePayload>;

    /// 创建只读视图
    pub fn read_view(&self) -> ReadView;
}
```

### Write Operations

```rust
impl AgentContext {
    /// 写入记录到指定类别
    /// 若 key 已存在则覆盖（记录 StateChangeLog）
    /// 若超过软限制则 warn! 日志，但仍写入
    pub fn set_record(
        &self,
        category: Category,
        key: String,
        value: serde_json::Value,
        source: String,
        iteration: usize,
    ) -> Result<(), ContextError>;

    /// 追加记录到指定类别（不覆盖）
    pub fn append_record(
        &self,
        category: Category,
        entry: RecordEntry,
    ) -> Result<(), ContextError>;

    /// 写入扩展内容
    pub fn add_extension(&self, content: ExtensionContent) -> Result<(), ContextError>;

    /// 设置最终响应
    pub fn set_response_payload(&self, payload: ResponsePayload) -> Result<(), ContextError>;

    /// 更新生命周期状态
    pub fn set_lifecycle_state(&self, state: LifecycleState) -> Result<(), ContextError>;
}
```

### Merge Operations

```rust
impl AgentContext {
    /// 将子 Agent Context 合并回主 Context
    ///
    /// 合并规则（FR-006, FR-018）：
    /// - 主 Context 已有数据不覆盖
    /// - 子结果以完整委派链 ID 路径为前缀存入（如 `sub-001/sub-002/{category}/`）
    /// - 冲突数据（同名 key）存入 `{subagent_id}/{category}/conflict/` 子命名空间
    /// - 嵌套委派：子 Agent 可再创建子 Agent，合并时前缀自动追加
    ///
    /// 返回：
    /// - Ok(()) — 合并成功
    /// - Err(MergeFailed) — 合并过程中发生不可恢复错误
    pub fn merge_subagent_context(&self, subagent_id: String, child: &AgentContext) -> Result<(), ContextError>;
}
```

### Audit Operations (Tool/Skill/State Recording)

```rust
impl AgentContext {
    /// 记录 Tool 调用结果（FR-011）
    /// 内部通过 set_record(Category::ToolResults, ...) 写入
    /// 同时追加 ToolCallRecord 到审计日志
    pub fn record_tool_call(
        &self,
        tool_name: String,
        arguments: serde_json::Value,
        result: Option<serde_json::Value>,
        status: ToolCallStatus,
        started_at: SystemTime,
        completed_at: Option<SystemTime>,
    ) -> Result<(), ContextError>;

    /// 记录 Skill 执行结果（FR-011）
    /// 内部通过 set_record(Category::Extensions, ...) 写入
    /// 同时追加 SkillExecutionRecord 到审计日志
    pub fn record_skill_execution(
        &self,
        skill_name: String,
        input: serde_json::Value,
        output: Option<serde_json::Value>,
        status: SkillExecutionStatus,
        started_at: SystemTime,
        completed_at: Option<SystemTime>,
    ) -> Result<(), ContextError>;

    /// 获取审计日志（ToolCallRecord + SkillExecutionRecord + StateChangeLog）
    pub fn get_audit_log(&self) -> Vec<AuditRecord>;
}
```

### Prompt Building

```rust
impl AgentContext {
    /// 构建 Prompt 数据，按固定优先级裁剪
    /// 顺序: 用户输入(100%) > 最近推理 > Tool结果 > 实体 > 历史状态
    pub fn build_prompt_budget(&self, max_tokens: usize) -> Result<PromptData, ContextError>;
}
```

### Serialization

```rust
impl AgentContext {
    /// 创建快照（线程安全：获取所有分类读锁后深拷贝）
    pub fn snapshot(&self) -> ContextSnapshot;

    /// 序列化为 JSON
    /// 
    /// 输出包含 schema_version 字段（FR-020），初始版本为 "v1"
    /// 序列化过程中获取所有分类读锁，保证快照一致性
    pub fn to_json(&self) -> Result<String, serde_json::Error>;

    /// 从 JSON 反序列化
    /// 
    /// 若输入 JSON 的 schema_version 与 ContextConfig::schema_version 不匹配，
    /// 返回 ContextError::SchemaVersionMismatch（FR-020）
    pub fn from_json(json: &str, expected_version: &str) -> Result<Self, ContextError>;
}
```

---

## ReadView

只读视图，用于防止组件意外修改不应修改的数据。

```rust
pub struct ReadView<'a> {
    context: &'a AgentContext,
}

impl ReadView<'_> {
    pub fn user_input(&self) -> &UserInput;
    pub fn get_category(&self, category: Category) -> Vec<RecordEntry>;
    pub fn get_extensions(&self) -> Vec<ExtensionContent>;
    pub fn get_audit_log(&self) -> &[Record<Operation>];
}
```

---

## ContextError

```rust
pub enum ContextError {
    /// 类别不存在
    CategoryNotFound(Category),
    /// 记录不存在
    RecordNotFound { category: Category, key: String },
    /// 子 Agent 尝试写入黑名单类别
    BlacklistedCategoryWrite { category: Category, subagent_id: String },
    /// 生命周期状态转换非法
    IllegalStateTransition { from: LifecycleState, to: LifecycleState },
    /// 合并失败
    MergeFailed(String),
    /// 尝试写入敏感数据（FR-021）
    RejectedSensitiveData { field_name: String, reason: String },
    /// 子 Agent 黑名单包含不允许的类别（如 UserInput）
    InvalidBlacklist { category: Category, reason: String },
    /// JSON schema 版本不匹配（FR-020）
    SchemaVersionMismatch { expected: String, found: String },
}
```

---

## AgentContextSyncHook

将 turn 状态同步到 Agent Context 的 Hook 实现。

**执行顺序约束（IL-002）**：AgentContextSyncHook 必须在其他 AgentHook（如 SDKCaptureHook）之后执行，确保同步状态时其他 hook 产生的副作用已被记录。

**失败处理约束（FR-019）**：Hook 执行失败 MUST 记录 error 日志但不得中断 AgentLoop。

```rust
pub struct AgentContextSyncHook {
    context: Arc<AgentContext>,
}

#[async_trait]
impl AgentHook for AgentContextSyncHook {
    /// 在 LLM 调用前同步当前消息和迭代信息到 Context
    async fn before_llm_call(&self, ctx: &mut AgentHookContext);

    /// 在 Tool 执行后记录 Tool 结果到 Context
    async fn after_tool_execution(&self, ctx: &mut AgentHookContext);

    /// 在每轮迭代后同步工具事件和状态到 Context
    async fn after_iteration(&self, ctx: &mut AgentHookContext);

    /// 在子 Agent fork 前记录委派信息
    async fn before_subagent_fork(&self, ctx: &mut AgentHookContext, subagent_id: &str);

    /// 在子 Agent 合并后记录合并结果
    async fn after_subagent_merge(&self, ctx: &mut AgentHookContext, subagent_id: &str);
}
```

---

## PromptData

Prompt 构建结果。

```rust
pub struct PromptData {
    pub user_input: String,
    pub recent_reasoning: Option<String>,
    pub tool_results: Vec<serde_json::Value>,
    pub entities: Vec<serde_json::Value>,
    pub history_state: Vec<serde_json::Value>,
    pub estimated_tokens: usize,
    pub truncated: bool, // 是否发生裁剪
}
```

---

## ResponsePayload

```rust
pub struct ResponsePayload {
    pub text: String,
    pub extensions: Vec<ExtensionContent>,
    pub object_refs: Vec<ObjectRef>,
    pub suggestions: Vec<String>,
}
```

---

## ObjectRef

```rust
pub struct ObjectRef {
    pub object_type: String,  // "game", "activity", "item" 等
    pub object_id: String,
    pub display_name: String,
    pub metadata: HashMap<String, String>,
}
```

---

## ExtensionContent & ExtensionType

动态扩展内容，属于结构化响应的一部分（FR-009）。

```rust
pub struct ExtensionContent {
    pub content_type: ExtensionType,
    pub data: serde_json::Value,
    pub render_hints: HashMap<String, String>,
}

pub enum ExtensionType {
    Card,       // 卡片（如游戏卡片、商品卡片）
    Image,      // 图片
    Suggestion, // 推荐问题
    Link,       // 链接
    Button,     // 按钮
    Table,      // 表格
    Chart,      // 图表
    ObjectRef,  // 业务对象引用
}
```

**与 FR-008 的关系**：FR-008 列出的 9 种响应类型中，"文本回答"存储在 `ResponsePayload.text`（LLM 生成的文本回复），不属于 `ExtensionType`。其余 8 种类型均对应 `ExtensionType` 枚举值。

**类型校验约束（Edge Case §Extensions）**：
- `ExtensionContent.content_type` 必须是预定义的 `ExtensionType` 枚举值之一
- `ExtensionContent.data` 的 schema 由调用方定义，Context 不验证内部结构
- `ExtensionContent.render_hints` 为可选渲染提示，不影响响应正确性

---

## ContextSnapshot

Context 在某一时刻的深拷贝快照，用于调试回放（FR-016）。

```rust
pub struct ContextSnapshot {
    pub context_id: String,
    pub snapshot_at: SystemTime,
    pub lifecycle_state: LifecycleState,
    pub user_input: UserInput,
    pub categories_snapshot: HashMap<Category, Vec<RecordEntry>>,
    pub extensions_snapshot: HashMap<String, ExtensionContent>,
    pub audit_log_snapshot: Vec<AuditRecord>,
    pub schema_version: String,
}
```

**线程安全约束**：`AgentContext::snapshot()` 获取所有分类的读锁后创建深拷贝，快照创建期间其他写入操作被阻塞。

---

## ContextConfig

```rust
pub struct ContextConfig {
    /// 各分类的软限制条目数
    pub soft_limits: HashMap<Category, usize>,
    /// 默认软限制（默认 1000）
    pub default_soft_limit: usize,
    /// Prompt 构建最大 token 数（默认 128000）
    pub prompt_max_tokens: usize,
    /// Token 估算函数
    pub prompt_token_estimation_fn: Option<Box<dyn Fn(&str) -> usize>>,
    /// 调试模式：启用时执行中断后保留快照（IL-004）
    pub debug_mode: bool,
    /// 序列化 schema 版本（默认 "v1"）
    pub schema_version: String,
}

impl Default for ContextConfig {
    fn default() -> Self;
}
```

---

## Sensitive Data Guard

Context API 在写入时自动检查敏感模式（FR-021）。

```rust
const SENSITIVE_FIELD_PATTERNS: &[&str] = &[
    "api_key", "token", "password", "secret",
    "credential", "authorization", "private_key",
];

// 在 set_record() 和 append_record() 内部:
fn is_sensitive_key(key: &str) -> bool;
fn is_sensitive_value(value: &serde_json::Value) -> bool;
```

**检查范围（FR-021 约束）**：
- `is_sensitive_key()` — 检查 key 字符串是否包含 SENSITIVE_FIELD_PATTERNS 中的任意子串（大小写不敏感）
- `is_sensitive_value()` — 仅检查 `serde_json::Value` 的顶层 key（若 value 为 Object 类型），不递归深入嵌套结构
- 若检测到敏感模式，返回 `ContextError::RejectedSensitiveData { field_name, reason }`
- 此检查为 API 级防护，与上游脱敏职责（Assumptions §敏感数据）为双重防护
