# Implementation Plan: Agent Runtime（Capability-based WASM Plugin Runtime）

> **范围更新（2026-07-23）：** 原 US6 管理端测试聊天、admin chat 表、`/api/admin-chat*`、`/api/chat/sessions*` 及 `web-admin` Chat UI 已由 `81a84fe` 移除，属于 Legacy / Superseded，禁止恢复。现行普通用户聊天位于 `web-user`，使用 `/api/assistant`、`/api/newsession`、`/api/messages` 和 `chat_*_user` 表；其接口契约由 `007-external-assistant-api` 与当前实现负责。

**Branch**: `004-agent-runtime` | **Date**: 2026-05-26 | **Spec**: [specs/004-agent-runtime/spec.md](file:///home/developer/agent/hive-claw/specs/004-agent-runtime/spec.md)
**Input**: Feature specification from `/specs/004-agent-runtime/spec.md`

## Summary

在 hive-claw 现有 axum/MySQL/Redis/Rustfs 栈上添加 Agent Runtime 的**管理面**：以 Extism WASM 为执行容器、宿主用一组受控 Capability（Host Function）为 Plugin 提供受限的资源访问；上层抽象顺序为 Plugin → Function → Tool/Skill → Agent；Agent 多层路由完成对话。所有越权调用在宿主层硬性拒绝并审计。`CompiledPlugin` 缓存避免每次重新编译 WASM；DAG-based Workflow 编辑器在 Web 上拖拽。

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
6. HTTP REST 管理端 + DAG 编辑器（reactflow）；不含已移除的管理端测试聊天。Agent Runtime 可被独立的普通用户 Assistant API 复用，但 HiveWeb/HiveGUI 产品边界不因此改变：HiveWeb 是云端 Agent，HiveGUI 是独立的桌面本地 Agent；二者只复用代码与契约，HiveGUI 不得请求或 fallback 到 HiveWeb。

## Technical Context

**Language/Version**：Rust 1.97.1（由 `rust-toolchain.toml`
`channel = "1.97.1"` 与 workspace `rust-version = "1.97.1"` 精确锁定，
CI 同时校验 `rustc` / `cargo` release；不使用浮动工具链或旧 MSRV）
（后端 / 宿主 runtime），TypeScript 5.x（Web）；WASM 插件作者可任选
Extism 支持的语言。
**Primary Dependencies**：
- 后端：`axum`（HTTP）、`extism` ≥ 1.x（WASM runtime，Rust host SDK）、`sqlx` (MySQL)、`redis`、`aws-sdk-s3`、`tokio`、`tower-http`
- LLM 客户端：**复用 workspace 现有 `crates/providers`**（Anthropic / Azure OpenAI / Bedrock / OpenAI Compat / OpenAI Codex / GitHub Copilot / Fallback；`LLMProvider` trait + `ProviderRegistry` + `ToolCallRequest`）。Agent Runtime 不引入新的 LLM 客户端依赖。Per-Agent 模型选择：`agents.model_preset` 字段在 runtime 解析阶段查启动期 registry；只有存在的 Agent DB 行中该字段为 NULL 才走全局默认 preset，显式未知名称 fail closed，缺失 Agent 行不得解释为 NULL。
- 前端：React + Ant Design + `reactflow`（DAG 编辑器）+ axios
- WASM 插件 ABI：[Extism PDK](https://extism.org/docs/concepts/pdk)
**Storage**：MySQL（元数据）；Rustfs/S3（WASM 文件 + 大对象；`AWS_S3_FORCE_PATH_STYLE` 默认 `true`，可按部署调整）；Redis（Instance Pool 元信息 + Capability 审计采样）
**Testing**：cargo test（含集成测试加载真实 Extism plugin）；Vitest + Testing Library；axe-core
**Target Platform**：Linux server；WASM 由 Wasmtime（Extism 内部默认）执行
**Project Type**：Web application（hiveweb crate 扩展） + 独立 plugin SDK 文档
**Performance Goals**：见 spec §SC（host_call p95 ≤ 5ms，`CompiledPlugin` 缓存命中调用 p95 ≤ 50ms；原管理端对话端到端指标属于 superseded 历史记录）
**Constraints**：
- Plugin 越权调用 100% 拒绝（spec SC-007）
- WASM 单次调用硬超时 30s（FR-030）、内存上限 128 MiB（FR-032）
- Agent 嵌套深度 ≤ 10（FR-023）
**Scale/Scope**：500 个 Plugin、2000 个 Function、50 个 Agent、并发对话 100，Instance Pool 大小 64（启动可调）

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

**Re-evaluated 2026-05-26 post-clarify-v4**（吸收 12 项 clarify 决议）：

✅ **Principle I - Code Quality & Maintainability**：cargo fmt / clippy `-D warnings` / ESLint / Prettier 沿用 003 既有 CI。
✅ **Principle II - Test-First Development (NON-NEGOTIABLE)**：在 tasks.md 中 Phase 2.5 显式排红灯测试任务（不重蹈 003 的 ⚠ 偏离 1）。现行覆盖：契约测试（host_call ABI、HTTP、`GET /api/agents/model-presets`）、集成测试（Capability 鉴权 + 子 Agent 不继承 / Workflow 拓扑 + 环检测 / Agent 路由 + 循环检测 / FallbackProvider 失败链 / `CompiledPlugin` 缓存命中 + fresh Store/Instance 隔离 / Plugin 软删除时引用阻塞）、组件测试（DAG 编辑器 + Skill markdown 编辑 + ModelPreset 下拉）。原 SSE Chat/Chat UI 测试任务仅保留为 superseded 历史记录。
✅ **Principle III - User Experience Consistency**：a11y / 错误码（含 5007 ModelPresetUnknown）/ 分页样式沿用 003 约定；DAG 编辑器需补 keyboard navigation（轮廓在 plan，验证在 tasks）。原管理端 SSE Chat UX 已 superseded。
⚠ **Principle IV - Performance & Efficiency**：现行 SC-005 / SC-006 的本地组件（host_call / Instance Pool 命中）应满足 < 200ms。原管理端 Chat 的 SC-010/LLM 端到端延迟仅作为 superseded 偏离 4 历史记录保留。
✅ **Principle V - Simplicity & YAGNI**：增量到 hiveweb crate；**复用 `crates/agent` / `crates/skills` / `crates/providers` 三个现有 crate，不再造编排核心、不引入 async-openai**；Plugin SDK 重用 Extism PDK，不自造 ABI；DAG 编辑器用 reactflow，不自造图渲染。
✅ **Principle VI - Observability & Structured Logging**：每次非 Hook host_call / Workflow 节点 / Agent 路由 / LLM 调用先 emit 含 request_id 的结构化 tracing，再通过单 worker 有界队列 best-effort 写入 `runtime_audit_logs`；队列满或 DB 失败不得阻塞主流程。Hook 执行保持 tracing-only。
✅ **Security Requirements**：Capability 零信任默认 deny；Plugin 不能访问宿主任何资源除非显式声明；子 Agent 不继承父的 permissions / model_preset（最小权限）；WASM 执行隔离（Wasmtime sandbox）；Plugin 上传走管理中心 JWT + System+ 角色鉴权；危险 capability 赋予 + `main` 编辑 = Super-only；`db.execute` 仅命名查询，自由 SQL 永不暴露。
✅ **Technology Stack**：Rust + axum + MySQL + Redis + Rustfs 符合宪法 v1.4.0；LLM 走 `crates/providers`（多 backend + FallbackProvider）；Extism 是新增的 WASM runtime（无对应宪法条款），登记在 Complexity Tracking 偏离 5。

**Gate Result**：CONDITIONAL PASS — 偏离 5（引入 Extism 新组件）仍为现役；偏离 4（原管理端 Chat 的 LLM 端到端延迟）仅保留历史。Phase 2.5 红灯测试必须先红再绿。

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
│   │   └── agent.rs         # /agents CRUD + 路由
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
│   │   └── agent.rs
│   ├── models/
│   │   ├── capability.rs
│   │   ├── category.rs
│   │   ├── tag.rs
│   │   ├── plugin.rs
│   │   ├── function.rs
│   │   ├── workflow.rs
│   │   ├── tool.rs
│   │   ├── skill.rs
│   │   └── agent.rs
│   ├── middleware/         # 复用 003 既有：auth/request_id/rate_limit
│   ├── utils/              # 复用 003 既有
│   └── lib.rs              # 增加 runtime 模块 pub mod
├── migrations/             # 实际注册链 V001–V033；V013 保留空号
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
│   │   └── AgentPage.tsx            # 树形 Agent 层级
│   ├── components/
│   │   ├── DagEditor/              # 基于 reactflow
│   │   ├── PluginUploader.tsx
│   │   ├── SchemaEditor.tsx        # JSON Schema 编辑/校验
│   │   └── CapabilityPicker.tsx
│   ├── services/                    # 每个新页面对应一份 service
│   └── hooks/
```

**Structure Decision**：扩展现有 `crates/hiveweb`（避免单一特性新建 crate 违反 Principle V 的"至多三 deployable"）；前端继续在 `web-admin/`。WASM 插件作者侧不在本仓内，作者使用任意 Extism PDK；hive-claw 仓提供 `examples/plugins/` 目录展示 Rust PDK 写法 — 由 T005（README）+ T138（smoke-plugin + weather demo）落地。

## Complexity Tracking

### 偏离 4 — Principle IV / 对话端到端延迟（历史，已 Superseded）

| 项 | 内容 |
| --- | --- |
| 现状 | 原管理端 Chat 的 SC-010 含 1 次 LLM 调用；该 UI/API 已移除，本表仅保留历史决策 |
| 偏离类型 | 不可压缩的外部依赖延迟 |
| 缓解方案 | LLM 调用前/后耗时分别打 tracing span 字段 `llm_ms` 与 `host_ms`；perf-evidence 把宿主侧 ≤ 200ms 与端到端 ≤ 8s 分列 |
| 退出条件 | 不再作为 004 现行 SLO；普通用户 Assistant API 如需端到端 SLO，由其所属特性另行定义 |

### 偏离 5 — 引入 Extism WASM runtime（宪法 v1.4.0 未列）

| 项 | 内容 |
| --- | --- |
| 现状 | 宪法 Technology Stack 未把 Extism / Wasmtime 列入 canonical 栈 |
| 偏离类型 | 引入新核心组件 |
| 触发原因 | spec 明确以"Capability-based with Extism WASM Plugin"为前提；自造 sandbox 风险与工作量远大于复用 Extism |
| 简化替代方案（已否决） | (a) Lua 插件 — 资源访问难做 capability 隔离；(b) 仅内置 Function — 失去用户上传扩展能力；(c) 自造 ABI — Extism PDK 生态成熟、跨语言、生产可用 |
| 缓解 | Wasmtime 是 BytecodeAlliance 工业级 sandbox；Plugin 调用必须经 Capability 鉴权；超时 / 内存上限硬性限制 |
| 退出条件 | 永久接受；需在 .specify/memory/constitution.md 下一次修订时把 Extism 列入 canonical 栈（独立 PR） |

## Startup Initialization Order（CHK234）

DB migration 是**服务启动之外的部署前置步骤**：生产环境由部署流程预先创建并升级表结构；开发和测试环境必须先执行 `cargo run -p hiveweb --bin migrate`。`hiveweb` 主服务在任何运行模式下都不执行 migration，也不执行 `CREATE TABLE`。

迁移版本的唯一真值是 `crates/hiveweb/migrations/*.sql` 与
`crates/hiveweb/src/bin/migrate.rs::MIGRATIONS`：当前为 V001–V033，
V013 无文件/无注册项，V032 创建 runtime audit，V033 规范 Workflow
timeout 默认值。旧文档中的“V019–V038 Agent Runtime 扩展”是已合并的
历史规划标签，不得据此新增或重命名 SQL。

hiveweb 启动期严格按以下顺序执行；监听前置失败必须 fail-fast，但退出机制按下述
typed 路径区分，不能把 LLM registry 错误统称为 panic：

1. **加载 env / dotenv** — `.env` 与系统 env 合并，校验必需变量（`DATABASE_URL` / `JWT_SECRET` / `LLM_PRESETS_PATH` 等）
2. **Capability 注册表 upsert** — 代码静态列表 → `INSERT ... ON DUPLICATE KEY UPDATE` 到 `capabilities` 表；DB 中存在但代码已移除的项仅 warn 不删
3. **Builtin Function upsert** — 5 个 builtin function（FR-010 v5）→ idempotent upsert 到 `functions` 表（`kind = 1`）
4. **Custom Function 索引** — 拉所有 DB 中 `kind = 2` 的 Function + 关联 Plugin metadata，构建 `Arc<HashMap<identifier, FunctionDef>>` 缓存
5. **llm_presets.toml 加载** — 解析所有命名 preset，校验**正好 1 个** `default = true`；启动期构造单 provider primary 或由 primary + `providers[1..]` 组成的 `providers::FallbackProvider`，并把完整 chain 缓存到 registry。fallback 节点按 `(preset name, provider ordinal)` 稳定寻址，与 model 字符串无关，因此不同 backend 可配置相同 model。所有真实 runtime 调用点统一通过 `LlmRegistry::build_chain` 克隆已注册的 `Arc`，不得逐次构造 provider、调用/保留 `build_primary` 或退化为 primary-only。preset 的 token/temperature 仅补调用方缺省字段，显式请求值在整条链保持不变。provider 默认要求 `api_key_env` 指向非空凭据；仅 `auth="none"` + 显式合法 http(s) `base_url` 可免认证，且不得同时声明 `api_key_env`
6. **ToolRegistry 装配** — builtin Tools（5 个）+ DB custom Tools + Workflow-wrapped Tools 全部注册到 `agent::ToolRegistry`
7. **SubagentManager / MemoryStore 初始化** — 从 DB 加载 Agent 树并初始化 `crates/agent::*` 运行结构；不加载已移除的 admin ChatSession
8. **Instance Pool 初始化** — 创建带 per-Plugin/global 容量、FIFO wait queue 与 `CompiledPlugin` 缓存的 Pool；容量只计算 `in_use + reserved`，idle 编译缓存不占 permit；lazy 编译，每次调用 fresh Store/Instance
9. **HTTP Router 装配** — middleware 链（request_id → CORS → rate_limit → auth）→ 现行管理端 API group
10. **后台任务启动** — audit_retention cron / pool `CompiledPlugin` idle reaper；保留 reaper shutdown handle，使其可在 graceful shutdown 中显式停止并等待结束
11. **HTTP server listen** — 在所有前置完成后才开始接 socket

启动期失败处理：
- env 缺失 / DB 不可连 / 所需表不存在 → 在监听前返回启动错误并非零退出；表
  不存在时先在开发/测试环境运行 `migrate`，生产环境修复预建 schema
- llm_presets 缺失/不可读、TOML 无效、default 数量不为 1、default provider
  或其凭据无效 → 只输出静态 `llm_registry_load_failed`，不含路径、provider
  详情或凭据，并在绑定 listener 前返回错误、非零退出；此路径不是 panic
- builtin function upsert 因 DB 唯一冲突失败 → panic（schema 错乱比启动失败更严重）
- 明确声明为不可恢复的 Plugin runtime config 解析错误可按现有配置路径 panic；
  不得把该行为扩展到 LLM registry
- 旧 capability 在 DB 中但代码已删 → warn 日志 + 继续启动
- llm_presets 中某个非 default preset 解析失败（含引用凭据缺失/为空）→ warn
  日志 + 跳过该 preset + 继续启动；只有 NULL 选择 default，任何仍显式引用被
  跳过/未知名称的运行请求返回 typed `ModelPresetUnknown`，不得静默兜底

运行期 LLM deadline 与可观测性：
- 每个 provider 节点最多 25 秒，非 Plugin 调用的完整 fallback chain 最多
  45 秒；`llm.invoke` 的完整 chain 最多 25 秒，以便在 30 秒 Plugin 外层预算内
  归还实例并完成审计
- 成功结果统一返回 `actual_model` / `fallback_used` / `reason`；preset 默认参数
  只填充调用方省略值，显式 `max_tokens` / `temperature` 跨 provider 保持不变
  （`llm.invoke` 显式 `max_tokens=0` 无效并返回 4001，不得当作省略值）
- tracing 与 best-effort runtime audit 同步记录上述字段。只有 provider 切换写
  `llm_fallback`；Workflow 等生成的本地文本兜底写 `llm_local_fallback`，
  `fallback_used=false`，不冒充 provider fallback

## Graceful Shutdown（CHK236）

HiveWeb 同时监听 SIGINT 与 SIGTERM。任一信号到达后按固定顺序执行：

1. 停止 accept 新连接；
2. drain 活跃 HTTP 请求，硬上限 30 秒；
3. drain 完成或超时后停止并等待 idle reaper；
4. drop Instance Pool 与全部 `CompiledPlugin` 缓存；
5. 关闭 DB pool 后退出。

30 秒到期必须取消剩余请求并继续资源清理，不得无限等待。原 admin SSE session
中断标记属于 superseded 历史合同，不参与 shutdown。

## Phase 0: Research

研究主题（research.md 完整给出 Decision / Rationale / Alternatives）：

1. **Extism Rust SDK 集成**：API、生命周期、host function 注册、错误传播
2. **Instance Pool 策略**：bb8 通用池 vs 自研 LRU；多 Plugin 共享池 vs 每 Plugin 独立池
3. **Capability ABI 设计**：单入口 `host_call(name, bytes)` vs 多 host function 一一对应；payload 编码（JSON / MessagePack / Protobuf）
4. **DAG 编辑器**：reactflow vs xyflow vs vis-network
5. **Workflow 执行器**：拓扑 BFS + 并行 + 错误传播；超时与中断
6. **Agent 路由决策**：纯 LLM tool-calling / 规则 / 混合
7. **LLM client**：OpenAI 兼容 / litellm-style 多 provider 抽象
8. **流式回复（历史，已 superseded）**：原管理端测试聊天的 SSE vs WebSocket vs Server-Streamed HTTP 选型
9. **JSON Schema 校验**：jsonschema crate + draft 2020-12
10. **聊天会话持久化（历史，已 superseded）**：原 admin chat 表选型；现行用户表由外部 Assistant API 维护

## Phase 1: Design

详见：
- `data-model.md` — 核心实体 + 实际物理迁移清单（V001–V033，V013 空号）
- `contracts/api.md` — REST 端点契约
- `contracts/host-functions.md` — `host_call(capability, payload)` 单入口 ABI + 各 capability payload schema
- `quickstart.md` — 起 infra + 上传 plugin + 注册 function + 验证 Agent；另说明普通用户 Assistant API 的边界

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
9. US6 管理端测试聊天 + SSE（历史，已 superseded；不实施、不恢复）
10. US7 Category / Tag
11. Polish — 文档 / 性能 / a11y / 安全审查
