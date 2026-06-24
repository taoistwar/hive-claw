# Feature Specification: Agent Context 能力设计

**Feature Branch**: `009-agent-context` | **Created**: 2026-06-02
**Status**: Draft
**Input**: User description — Design Agent Context as the unified state carrier for a single Agent execution lifecycle.

## Clarifications

### Session 2026-06-02

- Q: Context 数据模型应采用固定 Schema 还是灵活 Map？ → A: 半结构化：核心字段强类型 + 灵活 extensions 区
- Q: 子 Agent Context 可见性采用何种控制策略？ → A: 全可见 + 黑名单：默认全部可见，调用方指定不可读的字段
- Q: Context 容量超限时的裁剪策略？ → A: 软限制 + 告警日志：不拒绝写入，但记录告警日志供运维关注
- Q: 子 Agent 合并回主 Context 时发生键冲突如何处理？ → A: 主 Context 优先 + 子结果存入独立命名空间（可追溯来源）
- Q: Context 并发写入时采用何种一致性策略？ → A: 分类级别写入锁：不同类别并行写入，同类别内串行
- Q: LLM Prompt 构建时 Token 超限的裁剪优先级？ → A: 固定优先级层级：用户输入 > 最近推理 > Tool 结果 > 实体 > 历史状态
- Q: Context 中敏感数据的保护策略？ → A: 不处理：敏感数据由上游模块在写入 Context 之前自行脱敏
- Q: Agent 执行中断/超时时 Context 的生命周期处理？ → A: 立即销毁：执行中断后 Context 立即被清理释放，不保留任何信息

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Agent 执行过程中统一状态管理 (Priority: P1)

作为 Agent Runtime 的开发者，我需要在一个统一的 Agent Context 中保存和读取一次 Agent 执行过程中产生的所有中间状态（用户输入、实体识别、Tool 结果、Workflow 中间结果等），使得各个执行组件（Agent、Skill、Tool、Workflow、Sub Agent、LLM）能够通过受控方式访问和更新这些状态。

**Why this priority**: 这是 Agent Context 的核心能力，没有它后续所有特性都无法成立。它定义了 Context 的数据模型、生命周期和读写 API。

**Independent Test**: 可以独立构造一个 Agent Context 实例，写入用户输入和若干中间状态（如实体、Tool 结果），然后从 Context 中正确读取这些值。无需依赖完整的 Agent Runtime 或 LLM 服务。

**Acceptance Scenarios**:

1. **Given** 一个新建的 Agent Context，**When** 写入用户原始输入和会话元数据，**Then** Context 中可正确读取这些信息
2. **Given** 一个已包含用户输入的 Context，**When** 写入实体识别结果和 Tool 返回结果，**Then** 后续步骤可从 Context 读取到全部已写入的数据
3. **Given** 一个包含多步执行状态的 Context，**When** 查询某一类状态（如所有 Tool 结果），**Then** 返回对应类别的完整记录列表
4. **Given** 一个 Context 实例，**When** 多个执行步骤并发读取，**Then** 读取操作线程安全且不阻塞

---

### User Story 2 - Tool 和 Skill 通过 Context 写入执行状态 (Priority: P2)

作为 Tool 或 Skill 的开发者，我需要在执行过程中将结果写入 Agent Context（如识别出的实体、查询结果、推荐内容、执行状态），使得后续步骤可以直接复用，避免重复调用。

**Why this priority**: 这是 Context 作为"统一状态中心"的关键价值，实现 Tool/Skill 与后续步骤的解耦。

**Independent Test**: 构造一个 Context 实例和一个模拟 Tool，Tool 执行后将结果写入 Context 的特定分类区域，验证后续读取能获取到正确数据。

**Acceptance Scenarios**:

1. **Given** 一个 Tool 执行完毕后，**When** 将查询结果写入 Context 的结果存储区，**Then** 后续 Agent 推理步骤可从 Context 读取该结果
2. **Given** 两个 Tool 先后执行，**When** 第一个 Tool 的写入结果被第二个 Tool 读取，**Then** 第二个 Tool 无需重复调用第一个 Tool 的能力
3. **Given** 一个 Tool 写入执行状态（成功/失败），**Then** Context 的记录中可追溯该 Tool 的执行轨迹

---

### User Story 3 - 子 Agent 协作中的 Context 读取与合并 (Priority: P3)

当主 Agent 委派任务给子 Agent 时，子 Agent 能够读取部分 Context，产生新的状态数据，并在完成后将结果合并回主 Context。

**Why this priority**: 支持多层 Agent 协作，是 Agent Runtime 扩展性的基础能力。

**Independent Test**: 构造一个主 Context 和一个子 Agent Context，将主 Context 的部分数据传递给子 Context，子 Context 执行后合并结果回主 Context，验证数据完整性。

**Acceptance Scenarios**:

1. **Given** 主 Agent 创建一个子 Agent 并委派任务，**When** 子 Agent 读取主 Context 的全部数据（除黑名单指定字段外），**Then** 子 Agent 能获取到主 Context 中的用户输入、实体、Tool 结果等信息
2. **Given** 子 Agent 完成了自己的执行并产生了结果，**When** 将子 Context 合并回主 Context，**Then** 主 Context 中以子 Agent ID 为命名空间前缀存储子 Agent 的新增数据，主 Context 已有数据不被覆盖
3. **Given** 子 Agent 执行过程中尝试修改黑名单中的字段，**Then** 该写入操作被拒绝并记录告警

---

### User Story 4 - 从 Context 构建 LLM Prompt (Priority: P2)

作为 Agent Runtime 的 LLM 调用模块，我需要从 Context 中提取结构化数据（用户输入、已识别实体、Tool 结果、历史推理结果）来构建发送给 LLM 的 Prompt，而非让 LLM 直接读取全部 Runtime 状态。

**Why this priority**: Context 作为 Prompt 构建的唯一数据来源，保证了 LLM 调用的一致性和可控性。

**Independent Test**: 构造一个包含多种状态数据的 Context，调用 Prompt 构建方法，验证生成的 Prompt 包含预期的数据片段。

**Acceptance Scenarios**:

1. **Given** 一个包含用户输入、实体和 Tool 结果的 Context，**When** 调用 Prompt 构建方法，**Then** 生成的 Prompt 中包含这些结构化数据
2. **Given** 一个 Context 中有大量 Tool 结果，**When** 构建 Prompt 时超过 token 限制，**Then** 按固定优先级裁剪（用户输入始终保留 > 最近一轮推理 > Tool 结果 > 实体 > 历史状态），保留最关键的信息

---

### User Story 5 - 从 Context 构建最终响应 (Priority: P2)

Agent 执行完毕后，Response Builder 从 Context 中提取文本回答、卡片、图片、推荐问题、链接、按钮、表格、图表、业务对象引用等内容，构建统一的结构化响应返回给业务端。

**Why this priority**: Context 是最终响应的数据来源，保证了响应构建的一致性和扩展性。

**Independent Test**: 在 Context 中写入文本、卡片数据、推荐问题等内容，调用 Response Builder，验证生成的响应包含所有预期字段。

**Acceptance Scenarios**:

1. **Given** Context 中包含文本回答和一个游戏卡片扩展内容，**When** 构建响应，**Then** 响应同时包含文本和卡片数据
2. **Given** Context 中包含推荐问题和相关链接，**Then** 响应中正确包含这些结构化内容

---

### User Story 6 - 执行轨迹与可观测性 (Priority: P3)

作为运维和调试人员，我需要从 Context 中查看完整的执行轨迹（Tool 调用记录、Skill 执行记录、Agent 委派记录、状态变更记录），用于调试、回放、监控和审计。

**Why this priority**: 可观测性对于问题定位和执行分析至关重要，但不阻塞核心功能上线。

**Independent Test**: 在 Context 中记录若干执行步骤的轨迹信息，导出轨迹日志，验证包含完整的操作序列和时间戳。

**Acceptance Scenarios**:

1. **Given** 一个完整的 Agent 执行过程，**When** 查看 Context 的执行轨迹，**Then** 包含每一步的工具调用、状态变更和时间戳
2. **Given** 执行过程中发生错误，**Then** 轨迹中包含错误信息和发生时的 Context 快照

---

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST 提供 Agent Context 数据结构，能够保存一次 Agent 执行过程中的所有中间状态和最终结果
- **FR-002**: Context MUST 保存用户发起请求的元信息，包括用户原始输入、当前消息、会话信息、请求元数据
- **FR-003**: Context MUST 支持保存执行过程中的工作状态（实体识别结果、意图识别结果、Tool 返回结果、数据查询结果、Workflow 中间结果、Agent 推理结果）
- **FR-004**: Context MUST 支持跨执行步骤的数据共享，避免重复调用 Tool 或重复推理
- **FR-005**: Tool 和 Skill MUST 能够通过受控 API 将执行结果和状态写入 Context
- **FR-006**: Context MUST 支持子 Agent 默认读取全部 Context 数据（调用方可通过黑名单指定不可读字段），产生新数据，并在完成后以主 Context 优先 + 子结果独立命名空间的方式合并回主 Context
- **FR-007**: Context MUST 提供从结构化数据构建 LLM Prompt 的能力，当数据超过 token 限制时按固定优先级裁剪（用户输入 > 最近推理 > Tool 结果 > 实体 > 历史状态），而非暴露全部 Runtime 状态
- **FR-008**: Context MUST 支持保存最终响应内容，包括文本回答、卡片、图片、推荐问题、链接、按钮、表格、图表、业务对象引用等结构化响应
- **FR-009**: Context MUST 支持存储动态扩展内容（Extensions），这些内容属于结构化响应的一部分而非 LLM 文本回复
- **FR-010**: Context MUST 在多轮 Agent Loop（ReAct、Function Calling、Tool Calling）中保持状态一致性
- **FR-011**: Context MUST 记录执行轨迹，包括 Tool 调用记录、Skill 执行记录、Agent 委派记录、状态变更记录
- **FR-012**: Context 的数据变更 MUST 是线程安全的，采用分类级别写入锁策略支持并发写入与并发读取
- **FR-013**: Context MUST 提供只读视图机制，防止组件意外修改不应修改的数据
- **FR-014**: Context MUST 支持按类别查询数据（如查询所有 Tool 结果、所有实体、所有扩展内容）
- **FR-015**: Context 的生命周期 MUST 与单次 Agent 执行周期绑定，执行结束即销毁（debug_mode 启用时保存快照后销毁，见 IL-004）
- **FR-016**: Context MUST 支持序列化与反序列化，用于调试回放和持久化审计
- **FR-017**: Context 在生命周期状态为 Completed 或 Terminated 后，任何写入操作 MUST 被拒绝并返回 ContextError::IllegalStateTransition
- **FR-018**: Context MUST 支持嵌套子 Agent 委派（深度不限），每次合并时子结果均以其完整委派链 ID 路径（如 `sub-001/sub-002/{category}/`）为前缀存入
- **FR-019**: AgentContextSyncHook 的执行失败 MUST 记录 error 日志但不得中断 AgentLoop；AgentLoop 继续执行并尝试下一次迭代的状态同步
- **FR-020**: Context 序列化 JSON MUST 包含 schema_version 字段（初始版本为 "v1"），反序列化时若 schema_version 不匹配则返回明确的版本不兼容错误
- **FR-021**: Context API MUST 拒绝写入包含 credential 类型字段（如 api_key、token、password、secret）的数据，返回 ContextError::RejectedSensitiveData

### AgentLoop 集成要求

- **IL-001**: AgentLoop 初始化时（new 方法中）必须在创建 TurnContext 之前创建 AgentContext 实例
- **IL-002**: AgentContextSyncHook 必须在其他 AgentHook（如 SDKCaptureHook）之后执行，确保同步状态时其他 hook 产生的副作用已被记录
- **IL-003**: AgentLoop 发生 panic 或超时异常时，Context 的 Arc 引用必须被正确释放，确保无 dangling reference
- **IL-004**: 可选 debug 模式可通过 ContextConfig 启用，当 debug_mode=true 时，执行中断后的 Context 不被立即销毁，而是保存快照后进入 Terminated 状态

### Edge Cases

- Context 在并发写入时采用分类级别写入锁策略：不同类别（如 tool_results、extensions、entities）并行写入互不阻塞，同类别内串行写入保证一致性
- Context 中存储的数据量过大时采用软限制策略：不拒绝写入，但记录告警日志供运维关注；各执行组件应自行管理写入量以避免溢出
- 子 Agent 合并回主 Context 时发生键冲突：采用主 Context 优先 + 子结果存入独立命名空间（以子 Agent ID 为前缀），保证可追溯来源
- Context 中敏感数据（如用户 PII、凭证）的保护由上游模块负责，在写入 Context 之前完成脱敏；同时 Context API 内置拒绝机制（FR-021），双重防护
- Agent 执行被中断或超时时 Context 立即被清理释放（默认行为）；若启用 debug_mode 则保留快照（IL-004）
- Extensions 动态扩展内容的类型校验与 schema 验证
- 软限制告警日志 MUST 包含以下字段：category 名称、当前条目数、阈值、写入来源组件名、迭代轮次

### Key Entities

- **AgentContext**: 一次 Agent 执行的完整状态容器，采用半结构化设计：核心字段（用户输入、实体、Tool 结果等）强类型 + 灵活的 extensions 区域用于运行时自定义数据
- **UserInput**: 用户请求信息，包含原始输入文本、会话 ID、请求时间戳、请求元数据
- **ExecutionState**: 执行过程中产生的中间状态，按类别组织（entities、intentions、tool_results、query_results、workflow_results、reasoning_results）
- **ToolCallRecord**: Tool 调用记录，包含 Tool 名称、参数、返回结果、执行时间、成功/失败状态
- **SkillExecutionRecord**: Skill 执行记录，包含 Skill 名称、输入、输出、执行时间
- **AgentDelegationRecord**: Agent 委派记录，包含子 Agent 标识、传入数据、返回数据、执行时间
- **StateChangeLog**: 状态变更记录，包含变更时间、变更字段、旧值、新值、变更来源
- **ExtensionContent**: 动态扩展内容，包含内容类型（卡片/图片/推荐问题/链接等）、结构化数据、渲染提示
- **ResponsePayload**: 最终响应负载，包含文本回答、扩展内容列表、业务对象引用
- **ContextSnapshot**: Context 在某一时刻的快照，用于调试回放

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 开发者可以在 30 分钟内基于 Agent Context API 编写一个 Tool 并将执行结果正确写入 Context
- **SC-002**: Context 的单次读写操作 p95 延迟 < 1ms（本地内存操作，不含 LLM 调用）
- **SC-003**: 子 Agent 合并 Context 回主 Context 的操作在 10ms 内完成（典型数据量 < 100KB）
- **SC-004**: Context 的 Prompt 构建方法能在 5ms 内从典型执行状态（含 10 条 Tool 结果 + 5 个实体）生成 Prompt 数据
- **SC-005**: 并发 10 个组件同时读取 Context 时不发生数据竞争或阻塞
- **SC-006**: Agent 执行轨迹完整记录率 100%（所有 Tool 调用、Skill 执行、状态变更均有记录）
- **SC-007**: Context 序列化与反序列化后数据完整性 100%（无信息丢失）

## Assumptions

- Agent Context 仅关注单次 Agent 执行周期内的状态管理，不负责长期记忆存储、向量知识库管理、会话持久化、用户资料管理
- Context 的线程安全由分类级别写入锁保障（每个数据类别拥有独立的写入锁），支持不同类别的并发写入与全局并发读取
- Context 中的扩展内容（Extensions）类型由项目中的响应类型定义决定，不需要 Context 本身定义具体的响应格式
- LLM Prompt 构建是 Context 的辅助方法，实际 Prompt 模板由调用方或独立模块定义
- 子 Agent 的 Context 可见性采用全可见 + 黑名单模式：默认子 Agent 可读取主 Context 全部数据，调用方创建子 Agent 时可通过黑名单指定不可读的字段
- Context 的容量限制采用软限制策略：达到配置上限时不拒绝写入，但记录告警日志；具体容量阈值在实现阶段确定
- 敏感数据（用户 PII、凭证等）由上游模块在写入 Context 之前完成脱敏，Context 本身不处理敏感数据保护
- 已有的 `crates/agent` 中的 `ContextBuilder`（用于构建 LLM prompt）与本特性中的 Agent Context 是不同概念：前者是 prompt 组装器，后者是执行状态容器；两者需要协作但不应混淆
