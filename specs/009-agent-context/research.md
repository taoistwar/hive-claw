# Research: Agent Context 能力设计

## Phase 0: Technical Decisions

### Decision 1: Agent Context 数据结构的存储模式

**Decision**: 基于 Rust `HashMap` + `RwLock` 的分类级别并发存储

**Rationale**: 
- 现有 `crates/agent` 中已有 `AgentHookContext`（单轮迭代状态）和 `TurnContext`（单 turn 状态），但它们都是 per-turn/per-iteration 的短期结构
- Agent Context 需要跨整个 Agent 执行生命周期（多轮 loop）保持状态，与现有的短期上下文不同
- 使用 `Arc<RwLock<HashMap<Category, CategoryStore>>>` 实现分类级别并发：
  - 不同 category 的 `RwLock` 独立，读多写少场景下读锁可共享
  - 同类别内写操作串行化，避免竞争
  - 符合 spec 的"分类级别写入锁"要求

**Alternatives considered**:
- 单一全局 `RwLock<AgentContext>` — 写锁竞争大，不符合并发写入要求
- DashMap（并发 HashMap 库） — 引入第三方依赖，且无法实现"同类别串行"语义
- 通道（channel） + 单线程写入 — 架构复杂度高，延迟不可控

---

### Decision 2: 分类写入锁的具体实现模式

**Decision**: `Arc<RwLock<T>>` per category

**Rationale**:
- Rust 标准库 `RwLock` 满足读写锁语义
- 每个分类独立持有 `Arc<RwLock<CategoryData<T>>>`
- 读取时：获取对应分类的 `read()` 锁，多个读者可并发
- 写入时：获取对应分类的 `write()` 锁，同类别写入者串行
- 软限制告警：写入时检查条目数阈值，超限仅 `warn!` 日志

**Alternatives considered**:
- `Mutex<T>` per category — 读写互斥，读性能差
- 无锁结构（ArcSwap）— 写入复杂，不适合频繁修改场景

---

### Decision 3: 与现有 `AgentHookContext` / `TurnContext` 的集成策略

**Decision**: Agent Context 作为 `AgentLoop` 的伴随状态，通过 `TurnContext` 的扩展字段注入

**Rationale**:
- 现有 `TurnContext`（`loop_.rs:L274-304`）管理单个 turn 的状态（initial_messages、final_content、tools_used 等）
- 新增的 Agent Context 是跨 turn 的状态容器，生命周期比 TurnContext 长
- 集成方式：在 `AgentLoop` 启动时创建 Agent Context，每轮 turn 通过 hook 将 `TurnContext` 的关键信息同步到 Agent Context
- 在 `AgentRunner` 每轮迭代结束后，通过 `AgentHook.after_iteration()` 将工具调用结果、状态变更同步到 Context

**Alternatives considered**:
- 替代现有 TurnContext — 破坏现有架构，不兼容 crates/agent 的其他调用方
- 完全独立于 AgentLoop — 需要手动管理同步，易遗漏

---

### Decision 4: 子 Agent Context 合并策略

**Decision**: 子 Agent 拥有独立的 `AgentContext` 实例，合并时按分类独立合并

**Rationale**:
- 子 Agent 默认读取全部主 Context（黑名单模式），通过只读视图或浅拷贝实现
- 子 Agent 执行过程中写入自己的 Context
- 合并时：
  - 主 Context 已有的键不覆盖
  - 子 Agent 的新增数据以 `{subagent_id}/{category}/` 为前缀存入
  - 冲突数据存入 `{subagent_id}/{category}/conflict/` 子命名空间
- 符合 spec 的"主 Context 优先 + 子结果独立命名空间"要求

**Alternatives considered**:
- 共享同一个 Context 实例（引用传递）— 子 Agent 可直接修改主 Context，违反隔离
- 序列化传递 — 开销大，延迟高

---

### Decision 5: Prompt 构建的 Token 裁剪实现

**Decision**: Context 提供 `build_prompt_budget(max_tokens)` 方法，按固定优先级裁剪

**Rationale**:
- 裁剪顺序：用户输入(100%) > 最近一轮推理(100%) > Tool 结果(按相关性降序) > 实体(全部) > 历史状态(裁剪到 token 预算)
- 使用 `tiktoken` 或类似 token 计数器估算 token 数
- 与现有 `ContextBuilder`（`crates/agent/src/context.rs`）协作：ContextBuilder 从 Agent Context 提取结构化数据，组装为 prompt 文本
- 符合 spec 的"固定优先级层级"要求

**Alternatives considered**:
- 由 LLM 自行判断 — 额外调用一次 LLM，成本高
- 调用方配置优先级 — API 复杂度高，不符合 YAGNI

---

### Decision 6: 序列化与反序列化格式

**Decision**: JSON（`serde_json`）作为主要序列化格式

**Rationale**:
- 项目已有 `serde_json` 依赖
- JSON 格式对人类可读，便于调试回放
- 支持嵌套结构和动态类型
- 与现有的 `StateTraceEntry`（使用 `serde_json::Value`）保持一致

**Alternatives considered**:
- MessagePack/Protobuf — 更高效但不可读，不适合调试回放
- CBOR — 二进制格式，不适合运维人员直接查看
