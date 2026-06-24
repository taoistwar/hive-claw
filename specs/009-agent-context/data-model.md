# Data Model: Agent Context

## Entity Definitions

### AgentContext

**Description**: 一次 Agent 执行的完整状态容器。采用半结构化设计：核心字段强类型，extensions 区域灵活扩展。

**Fields**:
- `context_id: String` — Context 唯一标识（与 Agent 执行 ID 一致）
- `user_input: UserInput` — 用户请求信息
- `categories: HashMap<Category, CategoryStore>` — 分类级别存储
- `extensions: HashMap<String, ExtensionContent>` — 动态扩展内容
- `audit_log: Vec<AuditRecord>` — 执行轨迹记录（ToolCallRecord + SkillExecutionRecord + StateChangeLog 的联合枚举）
- `response_payload: Option<ResponsePayload>` — 最终响应
- `lifecycle_state: LifecycleState` — 生命周期状态（Active / Completed / Terminated / Merging）
- `created_at: SystemTime` — 创建时间
- `metadata: HashMap<String, String>` — 自定义元数据

---

### UserInput

**Fields**:
- `raw_text: String` — 用户原始输入文本
- `session_id: Option<String>` — 会话标识
- `message_id: Option<String>` — 消息标识
- `timestamp: SystemTime` — 请求时间戳
- `metadata: HashMap<String, String>` — 请求元数据（来源渠道、语言偏好等）

---

### Category (enum)

```rust
pub enum Category {
    Entities,           // 实体识别结果
    Intentions,         // 意图识别结果
    ToolResults,        // Tool 返回结果
    QueryResults,       // 数据查询结果
    WorkflowResults,    // Workflow 中间结果
    ReasoningResults,   // Agent 推理结果
    Extensions,         // 扩展内容的分类索引视图（数据实际存储在顶层 AgentContext.extensions 字段中）
    StateChanges,       // 状态变更记录
    SubagentResults,    // 子 Agent 结果（独立命名空间）
}
```

---

### CategoryStore<T>

**Fields**:
- `lock: CategoryLock<T>` — 分类级别读写锁封装（内部为 `Arc<RwLock<Vec<RecordEntry>>>`）
- `records: Vec<RecordEntry>` — 该分类下的记录列表
- `soft_limit: usize` — 软限制阈值（默认配置）
- `warning_emitted: bool` — 是否已发出超限告警

**Concurrency model**: 每个 Category 拥有独立的 `Arc<RwLock<Vec<RecordEntry>>>`。读操作获取 `read()` 锁（可并发），写操作获取 `write()` 锁（串行）。不同 Category 之间的锁互不影响。

---

### RecordEntry

**Fields**:
- `key: String` — 记录标识（如 tool name、entity name）
- `value: serde_json::Value` — 记录值
- `source: String` — 写入来源（tool name / skill name / subagent id）
- `timestamp: SystemTime` — 写入时间
- `iteration: usize` — Agent 执行迭代轮次

---

### ToolCallRecord

**Fields**:
- `tool_name: String` — Tool 名称
- `arguments: serde_json::Value` — 调用参数
- `result: Option<serde_json::Value>` — 返回结果
- `status: ToolCallStatus` — 状态（success / failure / timeout）
- `started_at: SystemTime` — 开始时间
- `completed_at: Option<SystemTime>` — 完成时间
- `error: Option<String>` — 错误信息

```rust
pub enum ToolCallStatus {
    Success,
    Failure,
    Timeout,
}
```

---

### SkillExecutionRecord

**Fields**:
- `skill_name: String` — Skill 名称
- `input: serde_json::Value` — 输入
- `output: Option<serde_json::Value>` — 输出
- `started_at: SystemTime` — 开始时间
- `completed_at: Option<SystemTime>` — 完成时间
- `status: SkillExecutionStatus` — 状态

```rust
pub enum SkillExecutionStatus {
    Success,
    Failure,
    Skipped,
}
```

---

### AgentDelegationRecord

**Fields**:
- `subagent_id: String` — 子 Agent 标识
- `task_description: String` — 任务描述
- `input_context: Vec<Category>` — 传入的类别（白名单/黑名单）
- `blacklisted_categories: Vec<Category>` — 不可读的类别黑名单
- `output_context: Option<serde_json::Value>` — 子 Agent 输出
- `started_at: SystemTime` — 委派开始时间
- `completed_at: Option<SystemTime>` — 完成时间
- `merge_status: DelegationMergeStatus` — 合并状态

```rust
pub enum DelegationMergeStatus {
    Merged,
    Failed,
    Pending,
}
```

---

### StateChangeLog

**Fields**:
- `timestamp: SystemTime` — 变更时间
- `category: Category` — 变更的类别
- `key: String` — 变更的键
- `old_value: Option<serde_json::Value>` — 旧值
- `new_value: serde_json::Value` — 新值
- `source: String` — 变更来源（组件名称）
- `iteration: usize` — Agent 执行迭代轮次

---

### ExtensionContent

**Fields**:
- `content_type: ExtensionType` — 内容类型
- `data: serde_json::Value` — 结构化数据
- `render_hints: HashMap<String, String>` — 渲染提示（显示位置、样式等）

```rust
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

---

### ResponsePayload

**Fields**:
- `text: String` — 文本回答
- `extensions: Vec<ExtensionContent>` — 扩展内容列表
- `object_refs: Vec<ObjectRef>` — 业务对象引用
- `suggestions: Vec<String>` — 推荐问题

---

### ContextSnapshot

**Fields**:
- `context_id: String` — Context 标识
- `snapshot_at: SystemTime` — 快照时间
- `lifecycle_state: LifecycleState` — 快照时的生命周期状态
- `user_input: UserInput` — 用户输入
- `categories_snapshot: HashMap<Category, Vec<RecordEntry>>` — 各类别快照
- `extensions_snapshot: HashMap<String, ExtensionContent>` — 扩展内容快照
- `audit_log_snapshot: Vec<Record<Operation>>` — 轨迹快照

**Usage**: 用于调试回放。通过 `AgentContext::snapshot()` 方法创建。

---

### LifecycleState (enum)

```rust
pub enum LifecycleState {
    Active,      // 执行中
    Completed,   // 正常完成
    Terminated,  // 异常终止（中断/超时）
    Merging,     // 子 Agent 合并中
}
```

---

### ContextConfig

**Fields**:
- `soft_limits: HashMap<Category, usize>` — 各分类的软限制条目数
- `default_soft_limit: usize` — 默认软限制（默认 1000）
- `prompt_max_tokens: usize` — Prompt 构建最大 token 数（默认 128000）
- `prompt_token_estimation_fn: Option<Box<dyn Fn(&str) -> usize>>` — Token 估算函数
- `debug_mode: bool` — 调试模式：启用时执行中断后保留快照而非立即销毁（IL-004）
- `schema_version: String` — 序列化 schema 版本（默认 "v1"，FR-020）

### SensitiveFieldPattern

**Description**: 用于识别 credential 类型数据的模式列表（FR-021）

**Patterns**: `api_key`, `token`, `password`, `secret`, `credential`, `authorization`, `private_key`

**Usage**: Context API 在 `set_record()` 和 `append_record()` 中检查 key 和 value 中是否包含这些模式，若匹配则拒绝写入并返回 `ContextError::RejectedSensitiveData`

---

## Relationships

```
AgentContext
├── 1:1 UserInput
├── 1:N CategoryStore (按 Category 枚举分区)
│   └── N RecordEntry
├── 1:N ExtensionContent (顶层 extensions)
├── 1:N Record<Operation> (audit_log)
├── 0:1 ResponsePayload
└── 1:1 LifecycleState

Sub Agent Delegation:
主 AgentContext ──(创建)──> 子 AgentContext (独立实例)
子 AgentContext ──(合并)──> 主 AgentContext (新增数据以完整委派链 ID 路径为前缀)
嵌套委派: sub-001 ──(创建)──> sub-002 ──(合并)──> sub-001 ──(合并)──> 主 Context
最终前缀: sub-001/sub-002/{category}/

Prompt Builder:
AgentContext ──(提取)──> PromptData ──(裁剪)──> 最终 Prompt
裁剪顺序: 用户输入(100%) > 最近推理 > Tool结果 > 实体 > 历史状态

Response Builder:
AgentContext ──(提取)──> ResponsePayload

Sensitive Data Guard:
上游组件 ──(尝试写入)──> Context API ──(检查 SensitiveFieldPattern)──> 匹配则拒绝 / 不匹配则允许
```

## State Transitions

```
LifecycleState:
  Active ──(正常完成)──> Completed
  Active ──(中断/超时)──> Terminated (默认立即销毁; debug_mode 时保存快照后终止)
  Active ──(子 Agent 开始)──> Merging
  Merging ──(合并完成)──> Active

写入操作有效性:
  Active ──(写入)──> 允许
  Completed ──(写入)──> ContextError::IllegalStateTransition
  Terminated ──(写入)──> ContextError::IllegalStateTransition
  Merging ──(写入)──> 仅允许 merge 操作，其他写入被拒绝
```

## Validation Rules

- UserInput.raw_text 不可为空字符串
- Category 必须是预定义的枚举值之一
- RecordEntry.key 在同一 Category 内必须唯一（允许追加模式覆盖）
- ExtensionContent.content_type 必须是预定义的枚举值之一
- ResponsePayload.text 可为空（纯扩展响应场景）
- 子 Agent 黑名单不得包含 "UserInput" 类别（用户输入始终对子 Agent 可见）
