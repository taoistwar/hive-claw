# Implementation Plan: Agent Runtime（Capability-based WASM Plugin Runtime）

**Branch**: `004-agent-runtime` | **Date**: 2026-05-26 | **Spec**: [specs/004-agent-runtime/spec.md](file:///home/developer/agent/hive-claw/specs/004-agent-runtime/spec.md)
**Input**: Feature specification from `/specs/004-agent-runtime/spec.md`

## Summary

在 hive-claw 现有 axum/MySQL/Redis/Rustfs 栈上添加 Agent Runtime 的**管理面**：以 Extism WASM 为执行容器、宿主用一组受控 Capability（Host Function）为 Plugin 提供受限的资源访问；上层抽象顺序为 Plugin → Function → Tool/Skill → Agent；Agent 多层路由完成对话。所有越权调用在宿主层硬性拒绝并审计。Plugin 实例池避免每次重新编译 WASM；DAG-based Workflow 编辑器在 Web 上拖拽。

**重要复用**：004 不再造 agent loop / tool registry / skills loader / multi-provider LLM。已在 workspace 中实现：
- `crates/agent`：`AgentRunner` + `loop_` 编排循环、`ToolRegistry` + `Tool` trait、`SkillsLoader`（markdown frontmatter 拼 system prompt）、`SubagentManager`（多层路由）、`MemoryStore` + `AutoCompact`、20+ 内置 Tool（shell/fs/web/mcp/search/notebook/...）
- `crates/skills`：embedded markdown 资产（`include_dir!`）
- `crates/providers`：`LLMProvider` trait + Anthropic/Azure-OpenAI/Bedrock/OpenAI-Compat/Codex/GitHub-Copilot + `FallbackProvider`

004 在 hiveweb 中新增的是**管理面**：
1. DB-backed 元数据 CRUD（Plugin / Function / Workflow / Tool / Skill / Agent / Category / Tag）
2. WASM Plugin 上传与 Extism 加载 + Instance Pool
3. Capability dispatcher（host_call 鉴权 + 转发 + 审计）
4. 把 DB 中的 Tool / Function 注册成 `agent::Tool` 注入 `ToolRegistry`
5. 把 DB 中的 Skill markdown 拼到 Agent 的 system prompt（沿用 SkillsLoader 思路）
6. HTTP REST + SSE 聊天 + DAG 编辑器（reactflow）

## Technical Context

**Language/Version**：Rust 1.85+（后端 / 宿主 runtime），TypeScript 5.x（Web）；WASM 插件作者可任选 Extism 支持的语言。
**Primary Dependencies**：
- 后端：`axum`（HTTP）、`extism` ≥ 1.x（WASM runtime，Rust host SDK）、`sqlx` (MySQL)、`redis`、`aws-sdk-s3`、`tokio`、`tower-http`
- LLM 客户端：**复用 workspace 现有 `crates/providers`**（Anthropic / Azure OpenAI / Bedrock / OpenAI Compat / OpenAI Codex / GitHub Copilot / Fallback；`LLMProvider` trait + `ProviderRegistry` + `ToolCallRequest`）。Agent Runtime 不引入新的 LLM 客户端依赖。Per-Agent 模型选择：`agents.model_preset` 字段在 runtime 解析阶段查 `ProviderRegistry`，未命中 / NULL → 走启动期全局默认 preset。
- 前端：React + Ant Design + `reactflow`（DAG 编辑器）+ axios
- WASM 插件 ABI：[Extism PDK](https://extism.org/docs/concepts/pdk)
**Storage**：MySQL（元数据）；Rustfs/S3（WASM 文件 + 大对象）；Redis（Instance Pool 元信息 + Capability 审计采样）
**Testing**：cargo test（含集成测试加载真实 Extism plugin）；Vitest + Testing Library；axe-core
**Target Platform**：Linux server；WASM 由 Wasmtime（Extism 内部默认）执行
**Project Type**：Web application（hiveweb crate 扩展） + 独立 plugin SDK 文档
**Performance Goals**：见 spec §SC（host_call p95 ≤ 5ms，Plugin 调用命中池 p95 ≤ 50ms，对话端到端 p95 ≤ 8s）
**Constraints**：
- Plugin 越权调用 100% 拒绝（spec SC-007）
- WASM 单次调用硬超时 30s（FR-030，待 clarify）、内存上限 128MB（FR-031，待 clarify）
- Agent 嵌套深度 ≤ 10（FR-023）
**Scale/Scope**：500 个 Plugin、2000 个 Function、50 个 Agent、并发对话 100，Instance Pool 大小 64（启动可调）

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

**Re-evaluated 2026-05-26 post-clarify-v4**（吸收 12 项 clarify 决议）：

✅ **Principle I - Code Quality & Maintainability**：cargo fmt / clippy `-D warnings` / ESLint / Prettier 沿用 003 既有 CI。
✅ **Principle II - Test-First Development (NON-NEGOTIABLE)**：在 tasks.md 中 Phase 2.5 显式排红灯测试任务（不重蹈 003 的 ⚠ 偏离 1）。覆盖：契约测试（host_call ABI、HTTP、`GET /api/agents/model-presets`、SSE chat）、集成测试（Capability 鉴权 + 子 Agent 不继承 / Workflow 拓扑 + 环检测 / Agent 路由 + 循环检测 / FallbackProvider 失败链 / Instance Pool 命中 + reset / Plugin 软删除时引用阻塞）、组件测试（DAG 编辑器 + 聊天窗口 + Skill markdown 编辑 + ModelPreset 下拉）。
✅ **Principle III - User Experience Consistency**：a11y / 错误码（含 5007 ModelPresetUnknown）/ 分页样式沿用 003 约定；DAG 编辑器需补 keyboard navigation（轮廓在 plan，验证在 tasks）；SSE chat 错误事件结构化。
⚠ **Principle IV - Performance & Efficiency**：SC-005 / SC-006 / SC-010 含跨 LLM 远端调用，依赖外部服务延迟，需在 perf-evidence 单独 disclaimer；本地组件（host_call / Instance Pool 命中）应满足 < 200ms。LLM 端到端延迟登记为偏离 4（同 003 偏离 4 的精神）。
✅ **Principle V - Simplicity & YAGNI**：增量到 hiveweb crate；**复用 `crates/agent` / `crates/skills` / `crates/providers` 三个现有 crate，不再造编排核心、不引入 async-openai**；Plugin SDK 重用 Extism PDK，不自造 ABI；DAG 编辑器用 reactflow，不自造图渲染。
✅ **Principle VI - Observability & Structured Logging**：每次 host_call / Workflow 节点 / Agent 路由 / LLM 调用 emit 结构化日志，含 request_id（沿用 003 的 middleware）；`runtime_audit_logs` 表新增 capability 维度 + 出入参摘要（脱敏）。
✅ **Security Requirements**：Capability 零信任默认 deny；Plugin 不能访问宿主任何资源除非显式声明；子 Agent 不继承父的 permissions / model_preset（最小权限）；WASM 执行隔离（Wasmtime sandbox）；Plugin 上传走管理中心 JWT + System+ 角色鉴权；危险 capability 赋予 + `main` 编辑 = Super-only；`db.execute` 仅命名查询，自由 SQL 永不暴露。
✅ **Technology Stack**：Rust + axum + MySQL + Redis + Rustfs 符合宪法 v1.3.0；LLM 走 `crates/providers`（多 backend + FallbackProvider）；Extism 是新增的 WASM runtime（无对应宪法条款），登记在 Complexity Tracking 偏离 5。

**Gate Result**：CONDITIONAL PASS — 偏离 4（LLM 端到端延迟外部依赖）+ 偏离 5（引入 Extism 新组件）记录于 Complexity Tracking。Phase 2.5 红灯测试必须先红再绿。

## Project Structure

### Documentation (this feature)

```text
specs/004-agent-runtime/
├── plan.md              # 本文件
├── research.md          # Phase 0：技术决策
├── data-model.md        # Phase 1：实体 + 关系 + DDL
├── quickstart.md        # Phase 1：开发上手
├── contracts/
│   ├── api.md           # HTTP REST 契约
│   └── host-functions.md # extism host_call ABI 契约
└── tasks.md             # /speckit-tasks 产出
```

### Source Code (repository root)

```text
crates/hiveweb/
├── src/
│   ├── api/
│   │   ├── capability.rs    # GET /capabilities — 列出 + 描述
│   │   ├── category.rs      # /categories CRUD
│   │   ├── tag.rs           # /tags CRUD
│   │   ├── plugin.rs        # /plugins CRUD + 文件上传
│   │   ├── function.rs      # /functions CRUD
│   │   ├── workflow.rs      # /workflows CRUD + 执行
│   │   ├── tool.rs          # /tools CRUD
│   │   ├── skill.rs         # /skills CRUD
│   │   ├── agent.rs         # /agents CRUD + 路由
│   │   └── chat.rs          # /chat/sessions + SSE
│   ├── runtime/
│   │   ├── mod.rs
│   │   ├── capability.rs    # Host capability 注册表 + 鉴权 + 派发
│   │   ├── pool.rs          # Extism Instance Pool（LRU）
│   │   ├── invoker.rs       # Plugin 调用入口（resolve + invoke + audit）
│   │   ├── workflow.rs      # DAG 拓扑执行器
│   │   ├── agent.rs         # Agent 编排骨架（接 crates/agent::AgentRunner 占位，T031）
│   │   ├── orchestrator.rs  # Agent 编排 + 路由决策（run_session + route_to_subagent + hard-rule 安全门，T118/T119）
│   │   └── llm.rs           # LLM client（OpenAI 兼容）
│   ├── services/
│   │   ├── capability.rs
│   │   ├── category.rs
│   │   ├── tag.rs
│   │   ├── plugin.rs
│   │   ├── function.rs
│   │   ├── workflow.rs
│   │   ├── tool.rs
│   │   ├── skill.rs
│   │   ├── agent.rs
│   │   └── chat.rs
│   ├── models/
│   │   ├── capability.rs
│   │   ├── category.rs
│   │   ├── tag.rs
│   │   ├── plugin.rs
│   │   ├── function.rs
│   │   ├── workflow.rs
│   │   ├── tool.rs
│   │   ├── skill.rs
│   │   ├── agent.rs
│   │   └── chat.rs
│   ├── middleware/         # 复用 003 既有：auth/request_id/rate_limit
│   ├── utils/              # 复用 003 既有
│   └── lib.rs              # 增加 runtime 模块 pub mod
├── migrations/             # 新增 V008–V0xx（详见 data-model.md）
└── tests/                  # 新增 contract_*/it_* （详见 contracts/）

web/
├── src/
│   ├── pages/
│   │   ├── CapabilityPage.tsx       # 只读列表 + 描述
│   │   ├── CategoryPage.tsx
│   │   ├── TagPage.tsx
│   │   ├── PluginPage.tsx
│   │   ├── FunctionPage.tsx
│   │   ├── WorkflowPage.tsx         # 含 DAG 编辑器
│   │   ├── ToolPage.tsx
│   │   ├── SkillPage.tsx
│   │   ├── AgentPage.tsx            # 树形 Agent 层级
│   │   └── ChatPage.tsx             # 测试聊天
│   ├── components/
│   │   ├── DagEditor/              # 基于 reactflow
│   │   ├── PluginUploader.tsx
│   │   ├── SchemaEditor.tsx        # JSON Schema 编辑/校验
│   │   └── CapabilityPicker.tsx
│   ├── services/                    # 每个新页面对应一份 service
│   └── hooks/
```

**Structure Decision**：扩展现有 `crates/hiveweb`（避免单一特性新建 crate 违反 Principle V 的"至多三 deployable"）；前端继续在 `web/`。WASM 插件作者侧不在本仓内，作者使用任意 Extism PDK；hive-claw 仓提供 examples 目录展示 Rust PDK 写法（[NEEDS CLARIFICATION：是否落地 examples/plugins/，含 1-2 个 demo？]）。

## Complexity Tracking

### 偏离 4 — Principle IV / 对话端到端延迟（SC-010 ≤ 8s）

| 项 | 内容 |
| --- | --- |
| 现状 | SC-010 含 1 次 LLM 调用，p95 受外部模型供应商延迟主导 |
| 偏离类型 | 不可压缩的外部依赖延迟 |
| 缓解方案 | LLM 调用前/后耗时分别打 tracing span 字段 `llm_ms` 与 `host_ms`；perf-evidence 把宿主侧 ≤ 200ms 与端到端 ≤ 8s 分列 |
| 退出条件 | 永久接受偏离；SLO 文档分离"宿主延迟"与"含外部 LLM 延迟" |

### 偏离 5 — 引入 Extism WASM runtime（宪法 v1.3.0 未列）

| 项 | 内容 |
| --- | --- |
| 现状 | 宪法 Technology Stack 未把 Extism / Wasmtime 列入 canonical 栈 |
| 偏离类型 | 引入新核心组件 |
| 触发原因 | spec 明确以"Capability-based with Extism WASM Plugin"为前提；自造 sandbox 风险与工作量远大于复用 Extism |
| 简化替代方案（已否决） | (a) Lua 插件 — 资源访问难做 capability 隔离；(b) 仅内置 Function — 失去用户上传扩展能力；(c) 自造 ABI — Extism PDK 生态成熟、跨语言、生产可用 |
| 缓解 | Wasmtime 是 BytecodeAlliance 工业级 sandbox；Plugin 调用必须经 Capability 鉴权；超时 / 内存上限硬性限制 |
| 退出条件 | 永久接受；需在 .specify/memory/constitution.md 下一次修订时把 Extism 列入 canonical 栈（独立 PR） |

## Startup Initialization Order（CHK234）

hiveweb 启动期严格按以下顺序执行；任一步失败 → panic 退出（fail-fast，不 warn-and-continue）：

1. **加载 env / dotenv** — `.env` 与系统 env 合并，校验必需变量（`DATABASE_URL` / `JWT_SECRET` / `LLM_PRESETS_PATH` 等）
2. **DB migrations** — `migrate.rs` 运行至最新 schema（V001..V018）
3. **Capability 注册表 upsert** — 代码静态列表 → `INSERT ... ON DUPLICATE KEY UPDATE` 到 `capabilities` 表；DB 中存在但代码已移除的项仅 warn 不删
4. **Builtin Function upsert** — 5 个 builtin function（FR-010 v5）→ idempotent upsert 到 `functions` 表（`kind = 1`）
5. **Custom Function 索引** — 拉所有 DB 中 `kind = 2` 的 Function + 关联 Plugin metadata，构建 `Arc<HashMap<identifier, FunctionDef>>` 缓存
6. **llm_presets.toml 加载** — 解析所有命名 preset，逐个调 `providers::make_provider` 构造 primary + `providers::FallbackProvider::new` 套上 fallback 链；校验**正好 1 个** `default = true`（违反 → panic）
7. **ToolRegistry 装配** — builtin Tools（5 个）+ DB custom Tools + Workflow-wrapped Tools 全部注册到 `agent::ToolRegistry`
8. **SubagentManager / MemoryStore 初始化** — 从 DB 加载 Agent 树 + ChatSession 摘要到 `crates/agent::*` 内存结构
9. **Instance Pool 初始化** — 创建空 `HashMap<PluginId, PluginPool>`；不预热（lazy 编译，命中 SC-005 冷启动预算）
10. **HTTP Router 装配** — middleware 链（request_id → CORS → rate_limit-with-SSE-bypass → auth）→ 各 API group
11. **后台任务启动** — chat_retention cron / audit_retention cron / pool idle reaper
12. **HTTP server listen** — 在所有前置完成后才开始接 socket

启动期任一步失败的处理：
- env 缺失 / DB 不可连 / migration 失败 / llm_presets 缺 default → **panic + 退出码 1**
- builtin function upsert 因 DB 唯一冲突失败 → panic（schema 错乱比启动失败更严重）
- 旧 capability 在 DB 中但代码已删 → warn 日志 + 继续启动
- llm_presets 中某个非 default preset 解析失败 → warn 日志 + 跳过该 preset + 继续启动（已有 default 兜底）

## Phase 0: Research

研究主题（research.md 完整给出 Decision / Rationale / Alternatives）：

1. **Extism Rust SDK 集成**：API、生命周期、host function 注册、错误传播
2. **Instance Pool 策略**：bb8 通用池 vs 自研 LRU；多 Plugin 共享池 vs 每 Plugin 独立池
3. **Capability ABI 设计**：单入口 `host_call(name, bytes)` vs 多 host function 一一对应；payload 编码（JSON / MessagePack / Protobuf）
4. **DAG 编辑器**：reactflow vs xyflow vs vis-network
5. **Workflow 执行器**：拓扑 BFS + 并行 + 错误传播；超时与中断
6. **Agent 路由决策**：纯 LLM tool-calling / 规则 / 混合
7. **LLM client**：OpenAI 兼容 / litellm-style 多 provider 抽象
8. **流式回复**：SSE vs WebSocket vs Server-Streamed HTTP
9. **JSON Schema 校验**：jsonschema crate + draft 2020-12
10. **聊天会话持久化**：MySQL JSON 列 / 独立 chat_messages 表 / Redis 暂存

## Phase 1: Design

详见：
- `data-model.md` — 10 个核心实体 + DDL（V008–V0xx）
- `contracts/api.md` — REST 端点契约
- `contracts/host-functions.md` — `host_call(capability, payload)` 单入口 ABI + 各 capability payload schema
- `quickstart.md` — 起 infra + 上传 plugin + 注册 function + 调聊天

## Phase 2: Tasks（不在本命令产出）

由 `/speckit-tasks` 生成。预期阶段：
1. Setup（infra 检查、reactflow 依赖、Extism 加入 Cargo.toml）
2. Foundational（runtime 模块骨架、Capability 注册表、Instance Pool、Workflow 执行器骨架、Agent 路由骨架）
3. Phase 2.5 Tests — 红灯先于实现（吸取 003 偏离 1 教训）
4. US1 Plugin CRUD + 上传 + 三维检索
5. US2 Function / Tool / Skill
6. US3 Workflow DAG
7. US4 Capability 鉴权
8. US5 Agent 编排
9. US6 测试聊天 + SSE
10. US7 Category / Tag
11. Polish — 文档 / 性能 / a11y / 安全审查
