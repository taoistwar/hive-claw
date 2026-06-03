# Implementation Plan: Agent Context 能力设计

**Branch**: `004-agent-runtime` | **Date**: 2026-06-02 | **Spec**: [specs/009-agent-context/spec.md](file:///home/developer/agent/hive-claw/specs/009-agent-context/spec.md)
**Input**: Feature specification from `/specs/009-agent-context/spec.md`

## Summary

在 `crates/agent` 中新增 Agent Context 模块，作为一次 Agent 执行生命周期内的统一状态容器。Agent Context 采用半结构化设计（核心字段强类型 + 灵活 extensions 区），分类级别写入锁支持并发读写，与现有 `AgentLoop` / `ContextBuilder` / `AgentHook` 机制集成，为 Tool、Skill、Sub Agent、Workflow 提供统一的读写状态 API。不涉及数据库存储（纯内存结构），不替代现有短期上下文（TurnContext / AgentHookContext），而是作为跨 turn 的长期状态载体。

## Technical Context

**Language/Version**: Rust 1.85+
**Primary Dependencies**:
- `serde` / `serde_json`（序列化 + JSON 格式）
- `tokio`（async runtime，与现有 AgentLoop 一致）
- 标准库 `std::sync::RwLock` / `Arc`（分类级别写入锁）
- 复用现有 `crates/agent`：`AgentLoop`、`AgentRunner`、`AgentHook`、`ContextBuilder`、`ToolRegistry`
**Storage**: N/A（纯内存结构，序列化用于调试回放，不持久化）
**Testing**: `cargo test`（unit + integration）
**Target Platform**: Linux server（与 crates/agent 一致）
**Project Type**: Rust library crate extension
**Performance Goals**:
- 单次读写 p95 < 1ms（内存操作）
- 子 Agent 合并 < 10ms（典型 < 100KB）
- Prompt 构建 < 5ms
**Constraints**:
- 分类级别写入锁：不同类别并发写入，同类别串行
- 软限制告警：不拒绝写入，记录 warn 日志
- 敏感数据由上游脱敏，Context 本身不处理
- 执行中断时 Context 立即销毁
**Scale/Scope**: 单次 Agent 执行生命周期；典型数据量 < 100KB；典型类别数 8-12

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

✅ **Principle I - Code Quality & Maintainability**：Agent Context 作为独立模块加入 `crates/agent`，遵循现有 Rust 模块规范（pub trait、pub struct、文档注释）。API 设计保持最小化：读写操作不超过 10 个核心方法。
✅ **Principle II - Test-First Development (NON-NEGOTIABLE)**：TDD 严格执行。先写 Context 的读写测试、并发测试、序列化测试、合并测试，再写实现。
✅ **Principle III - User Experience Consistency**：Agent Context 的 API 命名遵循现有 `crates/agent` 的命名约定（snake_case、动词开头的方法名如 `get_tool_results`、`set_entities`）。错误格式与现有 `AgentHookContext` 一致。
⚠ **Principle IV - Performance & Efficiency**：SC-002 / SC-003 / SC-004 均为内存操作延迟目标，应可满足。`RwLock` 在低竞争下开销极小。分类级别锁设计避免了全局锁竞争。
✅ **Principle V - Simplicity & YAGNI**：仅在 `crates/agent` 中新增一个模块（`context/` 子目录）+ 若干 struct。不引入新 crate、不引入新第三方依赖（`serde_json` 已存在）。Context 不包装数据库、不引入 KV store，纯内存。
✅ **Principle VI - Observability & Structured Logging**：Context 记录 StateChangeLog，写入时 emit 结构化日志（warn 级别的软限制告警）。与现有 `AgentHook` 的日志格式一致。
✅ **Security Requirements**：敏感数据由上游脱敏；子 Agent 黑名单模式支持最小权限；只读视图防止意外修改。Context 不存储凭证类数据。
✅ **Technology Stack**：纯 Rust（serde_json、RwLock），符合宪法 v1.3.0 的 canonical 栈。不引入新框架。

**Gate Result**: PASS — 无偏离。

## Project Structure

### Documentation (this feature)

```text
specs/009-agent-context/
├── plan.md              # 本文件
├── research.md          # Phase 0：技术决策
├── data-model.md        # Phase 1：实体 + 关系
├── quickstart.md        # Phase 1：开发上手
├── contracts/
│   └── api.md           # Context API 契约（Rust trait / struct）
└── tasks.md             # /speckit-tasks 产出
```

### Source Code (repository root)

```text
crates/agent/
├── src/
│   ├── context/
│   │   ├── mod.rs          # AgentContext 主模块，重导出
│   │   ├── core.rs         # AgentContext 结构体 + 生命周期管理
│   │   ├── category.rs     # Category 枚举 + 分类存储（CategoryStore<T>）
│   │   ├── lock.rs         # 分类级别写入锁封装（CategoryLock<T>）
│   │   ├── read_view.rs    # 只读视图（ReadView / ReadOnlyCategoryView）
│   │   ├── merge.rs        # 子 Agent Context 合并逻辑
│   │   ├── prompt.rs       # Prompt 构建 + Token 裁剪（build_prompt_budget）
│   │   ├── response.rs     # ResponsePayload 构建
│   │   ├── serialize.rs    # 序列化 / 反序列化（ContextSnapshot）
│   │   ├── audit.rs        # 执行轨迹记录（ToolCallRecord / SkillExecutionRecord / AgentDelegationRecord / StateChangeLog）
│   │   └── config.rs       # Context 配置（软限制阈值、分类默认值等）
│   ├── hook.rs             # 新增：AgentContextSyncHook — 将 turn 状态同步到 Agent Context
│   ├── loop_.rs            # 修改：AgentLoop 初始化 Agent Context 并注入到 TurnContext
│   └── lib.rs              # 新增 pub mod context; 重导出 AgentContext / ReadView 等
└── tests/
    ├── context_basic.rs    # 基础读写测试
    ├── context_concurrent.rs # 并发读写测试
    ├── context_merge.rs    # 子 Agent 合并测试
    ├── context_serialize.rs # 序列化测试
    └── context_prompt.rs   # Prompt 构建测试
```

**Structure Decision**: 在现有 `crates/agent` 中新增 `context/` 子目录（与 `tools/`、`hook/` 等平级），不引入新 crate。集成方式为扩展 `AgentLoop` 的初始化流程（创建 Agent Context 实例）和 hook 机制（每轮 turn 同步状态到 Context）。

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| 无 | 本特性无宪法偏离 | — |
