# Feature Specification: Agent Runtime（基于 Capability 的 WASM 插件 Agent 运行时）

> **范围更新（2026-07-23）：**
> - `RecommendedGame`、`recommended_games*` 及 `/api/recommended-games*` 相关管理和公开接口已废弃，仅兼容保留；不得新增调用或扩展。
> - 原 US6“管理端测试聊天”及其 admin session/SSE API、`web-admin` Chat UI 和 admin chat 表已由 `81a84fe` 移除，属于 **Legacy / Superseded** 历史设计；不得恢复 `/api/admin-chat*` 或 `/api/chat/sessions*`。
> - 现行聊天是面向普通用户的独立入口：`web-user` 调用 `/api/assistant`、`/api/newsession`、`/api/messages`，使用 `chat_sessions_user` / `chat_messages_user`；`/api/assistant` 对客户端返回 JSON，而非本规范旧设计的管理端 SSE。其接口细节由 `007-external-assistant-api` 与当前实现负责。
> - 产品边界保持不变：HiveWeb 是云端 Agent，HiveGUI 是独立的桌面本地 Agent；两者只允许复用代码与契约，HiveGUI 不得请求或 fallback 到 HiveWeb。

**Feature Branch**: `004-agent-runtime`
**Created**: 2026-05-26
**Last Updated**: 2026-07-24
**Status**: Implemented
**Input**: User description: 添加 Agent Runtime（Capability-based with Extism WASM Plugin），它包括能力管理、分类管理、标签管理、WASM 插件管理、函数管理、Workflow 管理、工具管理、技能管理、Agent 管理；原始输入还包含管理端测试聊天窗口，但该部分现已 superseded。

## Glossary

- **Capability（能力）**：受控的运行时资源（如网络访问、文件读写、对象存储、数据库、LLM 调用）。Plugin / Agent 必须显式声明其使用的 Capability 才能调用，未声明 = 调用即拒绝。
- **Plugin（插件）**：开发者上传的 WASM 二进制 + 元数据，包含 0 个或多个 export 函数；Plugin 本身不可直接被 Agent 调用，需通过 Function 关联。
- **Function（函数）**：可被 Tool / Skill / Agent 直接调用的最小执行单元，分"内置（系统代码实现）"和"定制（绑定 Plugin 的 export）"两类。
- **Workflow**：以 DAG 编排多个 Function 的执行流，输出最后一个节点的结果或聚合结果。
- **Tool（工具）**：对外暴露给 Agent 的可调用项，可包装单个 Function 或一段 Workflow，对外只展示 JSON Schema 入参/出参。
- **Skill（技能）**：上层封装，可调用 Function 或 Workflow；技能附带的描述与提示词供 Agent 决策时使用。
- **Agent**：自然语言交互单元，包含系统提示词 / 绑定的 Tools+Skills / 允许的 Capabilities / 路由规则。分入口（main，唯一）与专家（多层嵌套）。
- **Host Capability Runtime**：宿主进程实现的"能力门面"，所有 Plugin 通过 `host_call(capability, payload)` 调用，由宿主鉴权后转发到真实资源。

## Clarifications

### Session 2026-05-26

- Q: Agent 把请求路由到子 Agent 的决策机制采用哪种？ → A: LLM tool-calling 自决 + hard-rule 安全门（深度上限 / 循环检测 / 越权 capability 拒绝）
- Q: Capability `db.execute` / `db.query` 允许的 SQL 范围？ → A: 仅宿主预注册的命名查询/写入；自由 SQL 永不暴露
- Q: 聊天端点是否必须流式返回？ → A（历史，已 superseded）: 原管理端测试聊天曾决定强制 SSE；该端点已移除，不能据此恢复。现行 `/api/assistant` 返回 JSON。
- Q: 同 Plugin identifier 多版本如何处理？ → A: 多版本共存；唯一性 = `(identifier, version)`；Function.plugin_id 绑定具体版本主键；升级 = 新版本 + 新 Function 显式接入
- Q: LLM 客户端/供应商抽象如何选？ → A: 复用 workspace 已有的 `crates/providers`（提供 `LLMProvider` trait + Anthropic/Azure-OpenAI/Bedrock/OpenAI-Compat/Codex/GitHub-Copilot/Fallback 多 backend + ProviderRegistry + ToolCallRequest）。Agent Runtime 不引入 async-openai，新增的 `runtime/llm.rs` 仅作 thin adapter：通过 `providers::make_provider(&cfg)` 拿到 `Arc<dyn LLMProvider>` 后转发 chat / tool-calling。
- Q: LLM 调用失败时的回退策略？ → A: 强制使用 `providers::FallbackProvider` 链；启动期按 preset（`primary` + `fallback: [...]`）构造一次，runtime 不感知具体 backend。原“链全部失败后发管理端 SSE `error`”的呈现契约已 superseded；现行调用方按自身 API 契约接收错误。
- Q: Agent CRUD 权限矩阵？ → A: `main` 任何字段修改限 Super；非 main Agent CRUD = System+；不论哪个 Agent，permissions 含 `is_dangerous=1` 的 capability（`network.http` / `db.execute` / `secret.get`）必须 Super 提交。
- Q: MVP 内置 function 初始集合？ → A: 启动期注册 5 个胶水函数：`format_template`（模板填充）/ `json_parse` / `json_stringify` / `text_regex_match` / `chat_respond`（生成用户可见回复）。这些不依赖 WASM、不可删除、可被禁用。
- Q: 路由到子 Agent 后 host_call 鉴权看哪个 permission 集合？ → A: 当前执行中的 Agent 自己的 permissions；不继承父 Agent，也不取交集。Capability 始终按"最近的 Agent 框架"判断，符合最小权限 / zero-trust。
- Q: Instance Pool 中哪些对象允许跨会话 / 跨 Agent 复用？ → A: 只缓存已校验的 `CompiledPlugin`；每次调用都创建 fresh Wasmtime `Store` 与 fresh `Instance`，linear memory、mutable global 与 table 从不跨调用复用。
- Q: Skill 与 Tool 在 LLM 看到的工具列表中如何区分？ → A: 沿用已有 `crates/agent` 模式。Tool 注册到 `agent::ToolRegistry`，LLM 通过 OpenAI tool-calling 看到（含 JSON Schema 入参/出参）；Skill 是 markdown 内容，调用 = `SkillsLoader` 把内容拼进当前 Agent 的 system prompt，**不**出现在 LLM tools 列表。Workflow 在 runtime 内被包装成一个 Tool 注册（Workflow Wrapper）。
- Q: 既然 `crates/agent` / `crates/skills` / `crates/providers` 已实现 agent loop / tool registry / skills loader / multi-provider LLM，004 应该复用哪些？ → A: 复用 `crates/agent::{AgentRunner, ToolRegistry, SubagentManager, SkillsLoader, ContextBuilder, MemoryStore}` 与 `crates/providers::{LLMProvider, FallbackProvider}`；004 在 hiveweb 中新增的是**管理面**（DB-backed 元数据 + HTTP CRUD + DAG 编辑器 + WASM Plugin Extism 适配器 + Capability dispatcher），**不**再造 agent 编排核心。
- Q: 不同 Agent 是否允许选不同的 LLM 模型 / FallbackProvider 链？ → A: 是。`agents` 表加 `model_preset VARCHAR(64) NULL` 字段，对应 hiveweb 启动时从 `llm_presets.toml` 加载的命名 preset（每个 preset 内部封装一个启动期构造并缓存的 `providers::FallbackProvider`：primary + fallback 链）。只有 NULL 走全局默认 preset（启动校验必须正好 1 个 `default = true`）；显式非 NULL 名称若未知必须 fail closed，不得静默改用默认 preset。子 Agent 不继承父的 preset。
- Q: SSE 事件顺序 / 加载态 / 中途断开 UX 如何定义？（CHK003/004/048）→ A（历史，已 superseded）: 以下事件与管理端 UI 契约随原测试聊天一起移除，不再是现行验收条件。
- Q: 危险 capability 在 CapabilityPicker 中对 System 角色的行为？（CHK015）→ A: 不渲染（hidden）而非 disabled。Super 才看得到；后端在保存时再校验一遍角色，前端绕过仍会被 2001 拒绝。详见 FR-022 补强。
- Q: 全部 14 个错误码的用户可见文案如何统一？（CHK020）→ A: contracts/api.md §Errors 用三列表（code / 内部含义 / 用户文案）固化全部 14 项；前端 `web-admin/src/utils/error_messages.ts` 集中维护映射，组件只引该字典。
- Q: capability 鉴权失败的 audit 行为？（CHK088）→ A: 4030 / 4040 拒绝路径**必须**写 audit log（`event_type = capability_denied`）— FR-004 v7 已强化。
- Q: 危险 capability 撤销动作的角色门限？（CHK063）→ A: 与赋予一致，仅 Super 可执行（防 System 在 Super 不在场时绕审计降权后再改 system_prompt）— FR-022 v7。
- Q: network.http 的 SSRF 防护？（CHK076）→ A: dispatcher 解析 URL 后拒绝 IPv4/IPv6 私网段 + 云 metadata 端点；DNS 解析后重校验实际 IP（防 DNS rebinding）；仅放行 allowlist 域名（per-Agent 可收窄）— FR-001 v7。
- Q: WASM 上传期静态校验？（CHK080）→ A: magic bytes + 文件大小 + import 全在 host 注册表内 + 忽略 manifest 自提权字段 + sha256 计算 — FR-005 v7。
- Q: 子 Agent 错误如何向父上报？（CHK099）→ A: 仅暴露 `{code, message}` envelope；system_prompt / 内部 tool 名 / stack trace / plugin identifier 不外泄；audit/tracing 也只写静态错误分类和关联 ID，不写原始详情 — FR-025 v7。
- Q: route_to_subagent 是否允许跨级路由？（CHK100）→ A: 不允许；必须是当前 Agent 的直接子 Agent — FR-025 hard-rule 第 ④ 项。
- Q: SSE chat 端点的会话所有权？（CHK107）→ A（历史，已 superseded）: 原设计为 JWT.admin_id == session.admin_id；admin session 端点和表均已移除。现行用户聊天按 `user_id` 隔离，不存在 Super 读取他人会话的管理端例外。
- Q: Threat Model 是否在 spec 中文档化？（CHK112）→ A: 是。spec §Threat Model 新增 TM-1..TM-5 + Out of Scope —— 显式列出 5 类威胁与缓解 + 不纳入范围的项。
- Q: WASM 加载前 sha256 校验？（CHK116）→ A: 是。宿主每次从对象存储 GET 后重算 sha256 比对 DB 值；不一致 → 拒绝 + audit + 通知运维 — FR-029 v7。

## User Scenarios & Testing *(mandatory)*

### User Story 1 - 管理 WASM 插件资源 (Priority: P1) 🎯 MVP

管理员需要上传、维护、检索 WASM 插件。Plugin 是 Capability 模型的载体。

**Independent Test**：上传一个 Plugin → 列表可见 → 编辑元数据 → 软删除 → 在过滤器里"已删除"看到 → 查看其依赖关系（被哪些 Function 引用，不能硬删）。

**Acceptance Scenarios**:
1. **Given** 管理员登录为 System+ 角色, **When** 上传 WASM 文件 + 填写名称/标识符/manifest/版本/作者/仓库, **Then** Plugin 入库，WASM 文件落到对象存储，列表立即可见。
2. **Given** 一个 Plugin 已被 Function 引用, **When** 管理员尝试删除, **Then** 系统拒绝并提示"该 Plugin 被 {N} 个 Function 引用，无法删除"。
3. **Given** Plugin 列表中有 100+ 条, **When** 管理员按分类 + 标签 + 关键词组合检索, **Then** 系统返回符合全部条件的结果。
4. **Given** Plugin 已软删除, **When** 管理员尝试访问其详情, **Then** 详情仍可读但所有"使用该 Plugin 的入口"已不再展示该项。

---

### User Story 2 - 定义可被 Agent 调用的能力（Function / Tool / Skill）(Priority: P1)

管理员将 Plugin 的 export 函数注册为 Function；进一步包装成 Tool / Skill，让 Agent 可以"按描述"调用。

**Independent Test**：注册一个定制 Function（关联一个 Plugin export）→ 定义其入参 JSON Schema 与出参 JSON Schema → 在 Tool 里包装它 → 进入 Skill → 验证 Agent 列表里可见此 Skill 并显示描述、入参 schema。

**Acceptance Scenarios**:
1. **Given** 已存在的 Plugin, **When** 管理员注册一个定制 Function 并指定 export 名 + JSON Schema, **Then** Function 入库且与 Plugin 建立强引用关系。
2. **Given** 一个内置 Function 已被系统注册, **When** 管理员查看 Function 列表, **Then** 能区分"内置"和"定制"，内置不可被删除。
3. **Given** 一个 Function 已存在, **When** 管理员创建 Tool 并选"直接包装该 Function", **Then** Tool 的 schema 直接复用 Function 的入参/出参 schema。
4. **Given** 多个 Function 已存在, **When** 管理员创建 Tool 并以 DAG 编排它们, **Then** Tool 的入参为 DAG 入口的入参，出参为 DAG 终点的出参（或聚合后的形状）。
5. **Given** 一个 Skill 包含 1 个 Function 与 1 个 Workflow, **When** Agent 调用该 Skill, **Then** 运行时按 Skill 定义顺序执行（先 Workflow 再 Function 由 Skill 自行约定）。

---

### User Story 3 - 编排 Workflow (Priority: P2)

管理员在 Web 上拖拽 Function 形成 DAG，定义节点之间的输入输出映射。

**Independent Test**：在 Web 上画 3 节点的 DAG（A→B→C，并行 D）→ 保存 → 通过 API 触发 Workflow → 返回最终结果。

**Acceptance Scenarios**:
1. **Given** Workflow 编辑器为空, **When** 管理员拖入 2 个 Function 并连接 A→B, **Then** Web 上展示连接关系并允许配置入参映射（B 的入参 = A 的某个出参字段）。
2. **Given** Workflow 已保存, **When** 调用其执行 API, **Then** 系统按拓扑顺序执行节点，并发节点并行执行，并返回最终结果或第一个错误。
3. **Given** Workflow 包含一个引用了已被软删除 Plugin 的 Function, **When** 管理员尝试发布该 Workflow, **Then** 系统拒绝并指出哪个节点不可用。
4. **Given** Workflow 编辑中, **When** 出现环（A→B→A）, **Then** 系统在保存时拒绝并指出环路。

---

### User Story 4 - 通过 Capability 控制资源访问 (Priority: P1)

任何 Plugin 在运行时调用 `host_call(capability, payload)`，宿主必须按 Plugin 所属 Agent 当前授权的 Capability 集合判断放行。

**Independent Test**：创建一个不带 `network.http` 权限的 Agent → 让它调用一个内部用 `http_get` 的 Plugin → 调用立即被宿主拒绝，返回明确错误。

**Acceptance Scenarios**:
1. **Given** Agent 的 `permissions` 包含 `s3.read`, **When** Plugin 调用 `host_call("s3.read", ...)`, **Then** 调用透传到对象存储并返回结果。
2. **Given** Agent 的 `permissions` 不包含 `network.http`, **When** Plugin 调用 `host_call("network.http", ...)`, **Then** 宿主拒绝并返回错误码 4030（Capability denied），不发起任何外部请求。
3. **Given** Plugin 调用未注册的能力, **When** 宿主收到 `host_call("foo.bar", ...)`, **Then** 宿主拒绝并返回错误码 4040（Capability unknown）。
4. **Given** 一次 Plugin 调用持续 30 秒以上, **When** 调用未返回, **Then** 宿主中止 WASM 执行（超时），并把上下文写入审计日志。

---

### User Story 5 - Agent 多层编排与路由 (Priority: P2)

入口 Agent `main` 接收用户输入；可直接回答，也可路由到某个专家 Agent；专家 Agent 又可路由到自己的子 Agent，深度 ≤ 10。

**Independent Test**：创建 `main` → 子 Agent `coding-expert` → 孙 Agent `rust-expert`。用户问"如何写 Rust 异步函数"，期望最终由 `rust-expert` 应答。

**Acceptance Scenarios**:
1. **Given** 系统初始化, **When** 管理员查看 Agent 列表, **Then** `main` 一直存在且不可删除。
2. **Given** `main` Agent 有 1 个子 Agent, **When** 用户发起对话且输入匹配子 Agent 描述, **Then** `main` 将请求路由到子 Agent，子 Agent 的回答透传给用户。
3. **Given** Agent 层级已达 10 层, **When** 管理员尝试在第 10 层下再加子 Agent, **Then** 系统拒绝（"已达最大嵌套深度 10"）。
4. **Given** 一个 Agent 配置了 3 个 Skills + 2 个 Tools, **When** 该 Agent 决策需调用某 Tool, **Then** 运行时按 Tool 的 schema 校验 LLM 给出的入参后再执行。
5. **Given** 一个 Agent 没有 `llm.*` capability, **When** 运行时尝试让它调用模型, **Then** 调用被宿主拒绝（与 US4 同机制）。

---

### User Story 6 - 管理端测试聊天窗口（历史，已 Superseded）

该故事曾要求 `web-admin` 提供测试聊天窗口和 admin session SSE API。实现后来被移除，现行 004 不再包含此用户故事，也不得以兼容名义恢复 `/api/admin-chat*`、`/api/chat/sessions*`、`ChatPage` 或 `ChatStream`。

普通用户的现行聊天由 `web-user` 通过 `/api/assistant`、`/api/newsession`、`/api/messages` 使用，属于独立的外部 Assistant API 范围；它复用 Agent Runtime，但不构成管理端测试聊天。

---

### User Story 7 - 分类与标签维护 (Priority: P3)

管理员对 Plugin / Function 等用分类（树状）+ 标签（扁平）做组织化。

**Independent Test**：创建分类 "Coding/Rust" → 创建标签 "official"、"experimental" → 给一个 Plugin 同时关联 → 在搜索里按"分类=Coding 且标签=official"过滤生效。

**Acceptance Scenarios**:
1. **Given** 系统空白, **When** 管理员创建分类 + 标签, **Then** 可在 Plugin / Function 编辑表单中选择。
2. **Given** 标签已被 N 个 Plugin 使用, **When** 管理员尝试删除该标签, **Then** 系统提示"被 N 个对象使用"，要求确认级联清除关联或拒绝。

---

### Edge Cases

- 同 Plugin identifier 多版本共存；唯一性 = `(identifier, version)`；Function 显式绑定具体版本主键（已在 §Clarifications 锁定）。
- Plugin 文件在对象存储中丢失（外部清理）但 DB 中仍有记录：列表展示 + 加载时检测，触发"file missing"错误。
- Workflow 节点的入参映射缺失字段：保存时校验，提示节点 X 的输入 Y 未连线。
- Agent 路由进入死循环（A→B→A）：运行时检测同一会话路径，超 **5 跳**（默认，可配 `AGENT_MAX_HOPS` env）自动终止并写 audit。
- 并发同时编辑同一 Workflow / Agent：以 `updated_at` 作乐观锁；PUT 请求体须携带 client 读到的 `updated_at`，与 DB 不一致时返回 409 要求 client 重读后再保存。
- 内置 Function 与定制 Function 标识符冲突：禁止注册同名定制函数。
- WASM 编译失败的 Plugin：上传时即拒绝并报错；不写入对象存储。
- 大文件 Plugin：上限 **16 MB**（默认，可配 `PLUGIN_MAX_BYTES` env），超出拒绝。
- Capability `db.execute` / `db.query` 仅暴露宿主预注册的命名查询/写入；自由 SQL 永不可达 — 已在 §Clarifications 锁定。
- （历史，已 superseded）原管理端 SSE 中断恢复设计随测试聊天移除；现行用户聊天的失败、重试与持久化语义由外部 Assistant API 规范负责。

## Requirements *(mandatory)*

### Functional Requirements

**Capability 系统**
- **FR-001**: 系统必须暴露一组宿主级 Capability，包括但不限于：`network.http`、`fs.read`、`fs.write`、`s3.read`、`s3.write`、`db.query`、`db.execute`、`llm.invoke`、`secret.get`、`time.now`、`log.emit`。
  **`network.http` SSRF 防护**（hard requirement）：dispatcher 在转发前必须解析 URL 并拒绝以下目标 — ① 私网段（IPv4 `10/8` `172.16/12` `192.168/16` `169.254/16` `127/8` `0.0.0.0`；IPv6 `::1` `fc00::/7` `fe80::/10`）；② 云元数据端点（`169.254.169.254`、`metadata.google.internal`、`metadata.azure.com` 等）；③ DNS 解析后**重新**校验解析出的实际 IP（防止 DNS rebinding）；④ 仅放行宿主配置的 allowlist 域名（per-Agent 可进一步收窄）。出站错误必须类型化为 `Policy`（策略拒绝，永不重试）或 `ResolveUnavailable`（DNS 暂时不可用，可按固定策略重试），不得靠错误字符串分类。
  **Capability 级限流**（hard requirement）：单个宿主进程内，`network.http`
  同一 `(plugin_id, session_id)` 最多同时执行 8 个请求，permit 必须覆盖完整
  HTTP future（含取消）；`session_id = None` 使用一个共享 sentinel 桶，不得绕过
  限制。`log.emit` 使用容量 100、补充速率 100 token/s 的 per-Plugin 令牌桶。
  两者均在权限校验和 args 反序列化之后、handler 之前准入；非法 args 不消耗
  配额。`log.emit` 配额只限制 Plugin 日志 handler，限流拒绝本身仍按 FR-004
  逐次写安全 tracing audit。生产入口必须先取得 per-Plugin bucket mutex 再读取
  `Instant`；测试时钟及 bucket 的 `last_refill` / `last_seen` 必须单调，乱序时间戳
  不得使同一时间区间重复补充 token。
- **FR-002**: 所有 Capability 调用必须经 `host_call(capability_name, payload_bytes) → result_bytes` 单一 ABI 入口。Extism 构建器必须显式关闭 WASI（`with_wasi(false)`）；`wasi_snapshot_preview1` 等 WASI import 在实例化时必须失败。关闭 WASI 不得移除 Extism PDK 基础设施或已注册的 `extism:host/user.host_call`。
- **FR-003**: 宿主必须按"当前正在执行的 Agent 的 permissions 集合"鉴权；不在集合内的 capability 立刻拒绝。此鉴权适用于所有调用路径：LLM 调用（`llm.invoke`）、Tool 调用（host_call via invoker）、Skill 执行（底层走 ToolRegistry）、Workflow 节点执行、以及 `route_to_subagent` 内的间接调用。子 Agent 不继承父 Agent 的 permissions，也不与父取交集 — 每个 Agent 的能力集合独立、显式声明（最小权限 / zero-trust）。
- **FR-004**: 每次 Capability 调用、Plugin 调用、Agent 路由、LLM 调用和
  Workflow 节点执行都必须先写一条脱敏的结构化 tracing audit。事件仅保留
  allowlist 元数据：`request_id` / `session_id` 关联标识、可用的
  Agent / Function / Plugin id、capability、事件类型、结果、耗时、静态错误
  分类，以及不超过 1 KiB 的 capability-specific 摘要；正文、凭据、参数值、
  原始下游错误和原始 Workflow `node_key` 不得进入日志。
  **拒绝路径同样必须写**：`4030 Capability denied`、`4045 Capability
  unknown` 及 capability 限流拒绝都必须在返回前写恰好一条
  `event_type = capability_denied` / `outcome = denied` 安全事件。成功、失败、
  timeout 与拒绝必须使用同一公共审计收口，不能漏记。
  Agent permissions DB 查询失败必须在返回 `5000` 前写恰好一条安全
  `capability_call/error`。Plugin Invoker 的公共审计收口必须在 Plugin DB
  lookup 之前建立，并以 exactly-once guard 覆盖 Plugin 不存在、pool acquire、
  S3 load、sha256 mismatch、WASM compile、取消/panic 及正常 call/reset 的所有
  终止路径；这些事件不得包含 Plugin input、URL/payload、数据库/S3/Extism
  原始错误或哈希值。
  运行链必须显式传递不可反序列化且无 `Default` 的
  `RuntimeExecutionContext`，保留同一 `request_id` / `session_id`。普通调用在
  tracing 后把安全副本非阻塞地提交到容量有限的单 writer 队列，作为
  `runtime_audit_logs` 的 best-effort 副本；队列满、writer 关闭/缺失或 DB
  失败不得阻塞或改变业务结果。进入 Hook 后上下文永久降级为
  tracing-only，嵌套 Hook → Workflow → Plugin → Capability 不得重新启用 DB
  持久化。上述 enqueue / persist / tracing-only / drop / persist-failure
  计数必须通过 `GET /api/runtime/pool/stats` 暴露并在管理端告警。
  LLM 调用的结构化结果、tracing 与 runtime audit 必须使用同一组脱敏字段：
  `model_preset`、`actual_model`、`fallback_used`、`reason`、`llm_ms` 与
  `request_id`。`reason` 只能是宿主定义的静态分类，不能包含 provider 原始错误。
  从 primary 切到备用 provider 才写 `event_type=llm_fallback`；LLM 链失败后由
  Workflow 等调用方生成本地文本属于应用层 `llm_local_fallback`，
  `fallback_used=false`、`actual_model=NULL`，不得伪装为 provider fallback。

**Plugin 管理**
- **FR-005**: 系统必须支持上传 WASM 文件并填写：名称、标识符（唯一）、Manifest、Runtime 要求（Extism 版本）、版本号、作者、仓库地址。
  **上传期静态校验**（hard requirement）：
  - WASM magic bytes 检查（`\0asm` + version）— 拒绝非 WASM 二进制
  - 文件大小 ≤ `PLUGIN_MAX_BYTES`（默认 16 MB）
  - 扫描 `imports` 段：所有 host function 必须在宿主注册表内；出现未注册 import → 拒绝上传（防止 Plugin 试图绑定未来 capability）
  - 忽略 Plugin manifest 中的 `allowed_hosts` / `allowed_paths` 字段（防止 Plugin 通过 manifest 自我提权）
  - 计算并存储 sha256 + 文件大小
- **FR-006**: 系统必须支持对 Plugin 的添加、修改、软删除（标记 `deleted_at`）。
- **FR-007**: 系统必须在 Plugin 被任何现存 Function 引用时拒绝软删除。Function 仅支持硬删除且没有 `deleted_at` 列，因此引用检查统计该 Plugin 的全部 Function 行。
- **FR-008**: WASM 文件必须存储到 S3 兼容对象存储；DB 仅持有引用键 + 校验和。S3 客户端必须支持通过 `AWS_S3_FORCE_PATH_STYLE` 调整桶寻址方式；未设置时默认 `true`，以兼容 Rustfs / MinIO 自定义 endpoint，连接支持虚拟主机桶寻址的服务时可由用户设为 `false`。
- **FR-009**: 系统必须支持"分类 + 标签 + 关键词" 三维组合检索，关键词需覆盖名称/标识符/描述。

**Function 管理**
- **FR-010**: Function 分为"内置（builtin）"与"定制（custom）"两类；内置由系统代码注册，不可删除，可被禁用。MVP 内置集合：`format_template`、`json_parse`、`json_stringify`、`text_regex_match`、`chat_respond`（前 4 个是 Workflow 胶水，最后一个用于生成用户可见的最终回复）。
- **FR-011**: 定制 Function 必须强引用一个未删除的 Plugin 与其某个 export 名。
- **FR-012**: Function 字段：id、type、name、identifier（唯一）、description、input_schema（JSON Schema）、output_schema（JSON Schema）、关联 plugin_id、plugin_export、created_at、updated_at。
- **FR-013**: 系统必须在保存 Function 时校验 input/output schema 为合法 JSON Schema（draft 2020-12 或同等）。

**Workflow 管理**
- **FR-014**: Workflow 必须以 DAG 表示，节点是 Function 引用，边携带"上游某 output 字段 → 下游某 input 字段"映射。Workflow 支持分类关联（`category_id`）、标签（`tag_ids`）、起始输入 schema（`input_schema`）、起始节点描述（`start_description`）、结束输出 schema（`output_schema`）、结束节点描述（`end_description`）、执行所需 capabilities 聚合（`required_capabilities`）。Graph GET / PUT 必须通过虚拟 start 节点的 `position.start_description` 与虚拟 end 节点的 `position.end_description`，分别往返同一份 `workflows.start_description` 与 `workflows.end_description`；省略任一描述字段时必须保留原值，两个虚拟节点均不得写入 `workflow_nodes`。Web DAG 编辑器必须对称展示、编辑并序列化这两个虚拟节点描述。未声明 `output_schema` 时，单一终端持久化节点的输出直接作为 Workflow 公共输出；多个终端节点按 `node_key` 字典序返回稳定对象；显式 scalar schema 必须保留 scalar。HTTP `outputs` 与 UI End 节点只展示公共输出，诊断 `node_results` 不得混入 End 展示；所有公共输出及 `node_results` 每个节点值的顶层都必须剥离 `_agent_context_updates`。Metadata POST / PUT 的 `timeout_ms` 必须由 service 层强制在 **1000..=330000** 毫秒；POST 省略时默认 **33000** 毫秒。越界请求必须在任何 DB 写入或乐观锁 bump 前返回 400。
- **FR-015**: Web 必须提供可视化编辑器以拖拽节点与连线。DAG 本地草稿缓存升级时
  必须保留用户的节点、位置、连线与运行结果；`dag_editor_v2` 升级到 v3 时仍需拉取
  最新 Graph，仅为缺失的虚拟 start/end 描述补服务端值，成功写入 v3 后方可删除
  v2。只有最新 Graph 拉取成功且 v3 写入成功后才能完成迁移；拉取失败时继续保留并展示 v2 草稿，既不删除 v2 也不写 v3，以便下次重试。
- **FR-016**: 系统必须在保存时拒绝包含环的 DAG。
- **FR-017**: Workflow 执行必须按拓扑顺序，独立分支并行执行；任一节点失败终止流程并返回错误（含节点 id）。
- **FR-018 WorkflowNode**: 节点类型（`node_type`）包括 `function_node`（引用 Function）、`start_node`（起始节点）、`end_node`（结束节点）、`generate_answer_node`（LLM 生成节点，含 `node_config` JSON 存储 system_prompt、model_preset、history_window 等 LLM 生成配置）。`start_node` 和 `end_node` 无需引用 Function。

**Tool 管理**
- **FR-019**: Tool 字段：id、type（function / workflow）、name、identifier、description、input_schema、output_schema、引用的 function_id 或 workflow_id、source（`workspace` | `builtin`）、is_always（对所有 Agent 自动可用）、category_id、required_capabilities、created_at、updated_at。Tool 在 runtime 通过 `crates/agent::ToolRegistry` 暴露给 LLM；workflow-wrapped Tool 在 ToolRegistry 上同样表现为单一 `Tool` impl，内部 dispatch 到 Workflow 执行器。`source = 'builtin'` 的 Tool 不可编辑，只能包装 builtin Function。
- **FR-020**: 单 Function Tool 的 schema 必须等于其引用 Function 的 schema。
- **FR-021**: Workflow-wrapped Tool 的 input_schema = DAG 入口 Function 的 input_schema；output_schema 由编辑者显式声明（默认为 DAG 终点的 output_schema）。

**Skill 管理**
- **FR-022**: Skill 内容以 markdown 形式持久化（含可选 YAML frontmatter）。调用 Skill = 把其 markdown 内容拼接到当前 Agent 的 system prompt 末尾；Skill **不**作为 OpenAI tool 暴露给 LLM。复用 `crates/agent::SkillsLoader` 的现有模式。Skill 可在文本中以模板插值方式引用 Function / Workflow 调用结果，但 Function/Workflow 的执行最终走 ToolRegistry。Skill 字段新增：source（`workspace` | `builtin`）、is_always（对所有 Agent 自动加载）、category_id、required_capabilities。

**Agent 管理**
- **FR-023**: 入口 Agent 标识固定为 `main`，必须存在且不可删除（删除 API 必须拒绝）。对 `main` Agent 的任何字段修改（含 system_prompt / tools / skills / permissions）仅 Super 角色可执行；非 main Agent 的 CRUD = System 及以上；任何 Agent 的 permissions 含 `is_dangerous=1` 的 capability 时，保存动作仍要求 Super。
  **危险 capability 的赋予 + 撤销 + 调用** 都受 Super 关口：
  - **赋予**（PUT /api/agents 把 `is_dangerous=1` 的 capability 加入 permissions）→ 仅 Super
  - **撤销**（PUT /api/agents 把 `is_dangerous=1` 的 capability 从 permissions 移除）→ 仍仅 Super（避免 System 在 Super 不在场时"先撤后改"绕过审计）
  - **调用**（运行时 host_call 走该 capability）→ dispatcher 按当前 Agent.permissions 鉴权（与赋予者无关）
  CapabilityPicker UI 规则（与 003 "无权限 = 不渲染" 约定一致）：
  - **System 角色**打开 CapabilityPicker → `is_dangerous=1` 的项**不渲染**（不是 disabled）。System 看不到、点不到、也无法通过 DevTools 强行勾选并提交（后端再次校验拒绝）。
  - **Super 角色**打开 CapabilityPicker → 所有 capability 都渲染；dangerous 项加 ⚠ 标记 + tooltip 解释风险。
  - 后端是权威：即便前端被绕过强行 POST 含 dangerous capability 的请求，service 层在保存时按调用者角色再校验一次（非 Super → 2001 InsufficientPermission）。
- **FR-024**: Agent 可嵌套至多 10 层；新建子 Agent 时校验深度。
- **FR-025**: Agent 字段：id、name、description、created_at、updated_at、tools（多对多 Tool）、skills（多对多 Skill）、system_prompt、permissions（capability 名集合）、parent_agent_id（NULL 表示根）、`model_preset`（可选；指向 hiveweb 启动时从 `llm_presets.toml` 加载的命名 preset；**只有 NULL** 使用全局默认 preset；显式非 NULL 名称未注册时返回 5007 / typed runtime error 并 fail closed；子 Agent 不继承父的 preset）。
  对 `llm.invoke` 而言，上述 NULL 语义只适用于已读取到的 Agent DB 行；缺失 Agent 行必须在 registry/provider 前安全失败，不得把“没有行”当作 `model_preset=NULL`。
  HiveWeb 必须在 Router 构建和监听端口前加载该 registry，并在启动期构造、验证和缓存每个 preset 的完整 primary + fallback provider chain。`LlmRegistry::build_chain` 只解析 preset 并克隆缓存的 `Arc<dyn LLMProvider>`；不得逐次构造 provider，所有生产调用点不得使用或保留 `build_primary` 旁路。fallback 条目使用 `(LlmPresetName, provider ordinal)` 形成的稳定内部 identity 解析，identity 与 model 字符串无关；不同 backend / fallback 条目允许使用相同 model，禁止按 model 名反查 provider。
  preset 的 `max_tokens` / `temperature` 是调用未显式提供对应字段时的默认值；调用方显式值在 primary 与所有 fallback 节点保持不变，不得在切换 provider 时被 preset 或 fallback 条目覆盖。`llm.invoke` 显式 `max_tokens=0` 属于无效参数，必须返回 4001，不能被 provider 预处理误认为省略值。每个 provider 节点墙钟上限为 25 秒，普通调用的整条链上限为 45 秒；`llm.invoke` 位于 30 秒 Plugin 外层预算内，因此其整个 provider chain 上限为 25 秒。到达任一整链 deadline 后必须停止继续 fallback 并返回 typed timeout。
  每个成功 LLM 结果必须携带 `actual_model`、`fallback_used` 与可空 `reason`；primary 成功时 `fallback_used=false` 且 `reason=NULL`，备用 provider 成功时 `fallback_used=true` 且 `reason` 为静态切换分类。应用层本地文本兜底不属于 provider fallback，必须使用独立审计事件并保持 `fallback_used=false`。
  文件缺失/不可读、TOML 无效、default 数量不为 1 或 default provider 无效时，记录不含路径/配置/凭据的静态错误并非零退出，不得以空 registry 继续启动。provider 默认必须声明 `api_key_env`，其值必须是无首尾空白的非空环境变量名，且对应环境变量在启动时存在并含非空凭据。唯一免认证形式是 `auth = "none"` + 显式合法 `http://`/`https://` `base_url`，且禁止同时声明 `api_key_env`；单独省略 key 不表示免认证。无效的非 default preset 可记录静态告警并跳过；任何仍显式引用该名称的 Agent / Workflow / 测试入口必须失败关闭，不得使用 default。
- **FR-026**: Agent 路由由 LLM tool-calling 自决（系统组装 system_prompt + 子 Agent 列表 + tools/skills 后由 LLM 选择 `route_to_subagent(agent_id, reason)` / 直接回复 / 调用 Tool）。宿主在此之上叠加 hard-rule 安全门：① 深度 ≥ 10 拒绝路由；② 同会话路径出现循环立刻终止（最大跳次见 §Edge Cases）；③ 越权 capability 直接 4030 拒绝；④ `route_to_subagent(agent_id, ...)` 的 `agent_id` 必须是当前 Agent 的**直接子 Agent**（不能跨级跳，防止越级访问）。
  **子 Agent 错误的脱敏上报**：子 Agent 内部抛错时，宿主仅向父 Agent / 当前调用方暴露 `{ code, message }` envelope（message 为用户文案，见 contracts/api.md §Errors）；**不**暴露子 Agent 的 system_prompt、内部 tool 名、Plugin identifier、stack trace、内部 capability 名（防止信息泄漏给恶意构造提示词的用户）。audit/tracing 仅保留静态错误分类、`request_id` 与 allowlist 元数据；原始内部错误、stack trace、provider/DB/S3/Extism 文本不得写入任何 audit/tracing 事件。
- **FR-027**：（已合并入 FR-003）Agent 调用 LLM、Tool、Skill、Workflow 时均必须先通过 Capability 鉴权；越权立即返回错误。详见 FR-003 的适用范围条款。

**Legacy 管理端测试聊天（已移除）**
- **FR-028 / 原聊天 FR-029（历史，已 superseded）**: 原管理端测试窗口、admin 会话所有权、SSE 事件/并发/UX 契约均不再是现行需求。相关 API、UI 与表已移除，不得恢复 `/api/admin-chat*` 或 `/api/chat/sessions*`。
- **现行边界**: 普通用户聊天只通过 `web-user` 的 `/api/assistant`、`/api/newsession`、`/api/messages` 入口访问；会话按 `user_id` 隔离并存储在 `chat_sessions_user` / `chat_messages_user`。该入口不使用管理中心 JWT/Super 例外，且 `/api/assistant` 返回完整 JSON 消息，不暴露原管理端 SSE 契约。

**Runtime 实现**
- **FR-029**: WASM 执行使用 Instance Pool：缓存 sha256 已验证的 `CompiledPlugin` 以避免重复编译；每次调用必须创建 fresh Wasmtime `Store` 与 fresh `Instance`，不得复用 linear memory、mutable global 或 table。池大小可配置，超时空闲编译缓存由后台 reaper 回收。
  **加载前 sha256 校验**（hard requirement）：宿主从对象存储 GET WASM 文件后、编译实例前，必须重新计算 sha256 并与 DB 中存储值比对；不一致 → 拒绝实例化 + 写 audit + 通知运维（防止对象存储被外部篡改）。
  **Pool 容量与行为**（CHK211 / CHK213 / CHK214）：
  - 默认上限：每 Plugin 最多 8 个并发 invocation slot（`PLUGIN_POOL_MAX_PER_PLUGIN`），全局最多 64 个并发 invocation slot（`PLUGIN_POOL_MAX_TOTAL`）
  - 空闲回收：编译缓存闲置超过 10 分钟（`PLUGIN_POOL_IDLE_TIMEOUT_SEC`）由后台任务释放
  - **Pool 满时 acquire 行为**：先按 FIFO 等待可用 invocation slot 最多 5 秒（`PLUGIN_POOL_ACQUIRE_TIMEOUT_MS`）；超时 → 返回 `5009 PoolBusy` 错误而非无限阻塞当前 Agent 运行
  - 等待开始即增加 `wait_count`；每 Plugin 与全局容量的占用/释放、FIFO waiter 唤醒及调用取消必须原子且 cancellation-safe，不得泄漏 permit 或越过上限
  - per-Plugin/global 容量只计算 `in_use + reserved` invocation slots；`idle` 仅表示可复用 `CompiledPlugin` 缓存，不占 permit，也不得阻止其他 Plugin 获得全局 slot
  - `cache_misses` 只在真实 Wasmtime 编译成功后增加；编译失败不增加，从已有 `CompiledPlugin` 创建 fresh Store/Instance 也不增加。`created_total` 只在 fresh Store/Instance 成功创建后增加；编译或实例化失败均不增加
  - idle reaper 在服务启动时创建、按配置周期运行，并在 graceful shutdown 时停止；只淘汰超过 idle timeout 且未被调用持有的 `CompiledPlugin`
  - **兼容指标**：现有 `/api/runtime/pool/stats` 中的 `reset_failures` 字段仅为响应兼容保留；fresh Store/Instance 设计不执行 runtime instance reset，因此该字段预期恒为 `0`，不得再以它驱动告警或审计
  - **调用方取消处理**：`Invoker::invoke` 一旦 checkout 实例即建立 pool-slot cancellation guard。调用方 abort/cancel 整个 future 时，guard 必须取消正在运行的 WASM、异步释放 `in_use`，并让 detached blocking task 仅 drop 实例；该实例不得回 idle，计数不得二次 release。Pool 的 inner/per-plugin 与 global metrics 必须在取得全部 mutex 后才一起突变，取消不得留下半更新。
- **FR-030**: 单次 Plugin 调用必须有硬超时，默认 **30 秒**（可配 `PLUGIN_CALL_TIMEOUT_MS` env），超时强制中止并归还实例位。
  **双层 timeout 实现**（CHK215）：① Wasmtime **fuel-based budget** 防止纯 CPU 死循环（按指令计数中断；`PLUGIN_CALL_FUEL` 为非零 `u64`，默认 `10000000000`）；② `tokio::time::timeout` 包住 blocking call 的 JoinHandle，默认墙钟预算 `PLUGIN_CALL_TIMEOUT_MS=30000`。外层到期时宿主必须显式调用 Extism `CancelHandle::cancel()`、立即释放池占用计数并返回 5004；fuel 耗尽同样返回 5004。两种路径都丢弃实例，不得回池或二次 release。Extism epoch cancellation 不能抢占正在执行的同步 Rust host handler，因此每个 capability I/O handler 仍必须配置自己的操作级 timeout。
- **FR-032**: 单次 Plugin 调用必须有内存上限，默认 **128 MiB**（可配 `PLUGIN_CALL_MAX_MEMORY_MB` env），超限中止。Extism `Manifest::with_memory_max` 的单位是 64 KiB WebAssembly page，因此宿主必须使用 `max_pages = memory_mb × 16`（128 MiB = 2048 pages），不得把 byte 数直接传入。内存超限属于 Plugin runtime failure，返回业务码 5000 + 安全 tracing + bounded best-effort DB runtime audit；5004 仅用于 timeout。任何 trap/OOM 对应的 fresh Store/Instance 都必须直接丢弃，不得进入任何缓存。

**Required Capabilities 声明**
- **FR-033**: Function、Tool、Skill、Workflow 均可声明 `required_capabilities`（JSON 数组，元素为 capability name）。声明表示该实体在执行期间需要调用的 capability 集合。系统在以下场景强制校验：
  - Function 创建时校验 `required_capabilities` 中的每一项必须属于 `capabilities` 表（service 层 lookup，不存在 → 5002）
  - Tool 包装 Function/Workflow 时，Tool 的 `required_capabilities` 必须为被包装实体的超集（service 层校验，缺项 → 5002）
  - Agent 绑定 Tool/Skill 时不校验（运行时由 capability dispatcher 按 Agent.permissions 鉴权，越权 → 4030）

**RecommendedGame 管理**
- **FR-034**: 系统支持维护推荐游戏列表，每条包含：名称、回复内容、推荐理由、游戏唯一标识（game_id）、游戏名称、标签（如"运营推荐/新游上线/本周热玩"）、游戏类型、推荐图片地址、排序值（sort_value）。支持按 sort_value 排序返回，game_id 全局唯一。

### Key Entities

- **Capability**：固定枚举集合，由宿主代码定义（真值源）。启动期将代码列表 upsert 到 `capabilities` 表（仅含描述 + is_dangerous + category_id 元数据，供 UI 渲染）；DB 中存在但代码已移除的项仅 warn 不删。
- **Category**：树状分类，自引用 parent_id，支持 Plugin / Function / Tool / Skill / Workflow / Capability 分组。
- **Tag**：扁平标签集合。
- **Plugin**：WASM 二进制 + 元数据。多对一 Category；多对多 Tag。
- **Function**：可执行单元。多对一 Plugin（仅定制）；多对一 Category；多对多 Tag；声明 required_capabilities。
- **Workflow**：DAG。多对一 Category；多对多 Function（通过 WorkflowNode）；声明 input_schema / output_schema / required_capabilities。
- **WorkflowNode + WorkflowEdge**：DAG 的节点 / 边。节点类型含 function_node / start_node / end_node / generate_answer_node（后者含 node_config JSON）。
- **Tool**：包装层。引用 Function 或 Workflow；声明 source（workspace | builtin）/ is_always / category_id / required_capabilities。
- **Skill**：上层封装。声明 source（workspace | builtin）/ is_always / category_id / required_capabilities。引用 Function 或 Workflow + 自有描述/示例。
- **Agent**：交互单元。自引用 parent_agent_id；多对多 Tools；多对多 Skills；Set 列存 permissions；可选 model_preset。
- **ChatSessionUser + ChatMessageUser（外部用户聊天，后续特性所有）**：普通用户会话与消息，分别存于 `chat_sessions_user` / `chat_messages_user` 并按 `user_id` 隔离；004 不提供 admin chat 实体。
- **AuditLog（runtime）**：tracing 是所有 runtime 事件的权威审计流；非 Hook
  事件另有一个有界、非阻塞、best-effort 的 `runtime_audit_logs` 持久副本。
  Hook 保持 tracing-only。
- **Admin + AdminAuditLog**：管理员账户与操作审计（继承自 003-admin-center）。
- **LoginRecord**：登录记录（继承自 003-admin-center）。
- **RecommendedGame**：推荐游戏列表，含排序和分类标签。
- **Dashboard**：统计聚合视图（plugin/function/workflow/agent/tool/skill/现行 user chat 计数 + 最近活动）；不包含已删除的 admin Chat。

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 上传一个 1MB 内 Plugin → 入库 + 落对象存储完成耗时 ≤ 5 秒（p95）。
- **SC-002**: Plugin 列表（≤ 500 条）+ 三维过滤 + 全文检索 p95 ≤ 1 秒。
- **SC-003**: Workflow 编辑器在 ≤ 50 节点的图上保存/校验 p95 ≤ 1 秒。
- **SC-004**: 单次 host_call 鉴权 + 转发开销 p95 ≤ 5 毫秒（不含目标资源耗时）。**度量边界**（CHK225）：从 dispatcher 收到 envelope 字节起，到准备调用 capability handler 之前（含 JSON 反序列化 / Agent.permissions 查询 / capability 名查表）；不含 handler 内部的网络 / DB / S3 调用耗时。压测工具：tokio `Bencher` 直接调 dispatcher，绕过 HTTP 层。
- **SC-005**: 在 `CompiledPlugin` 缓存命中的情况下，一次 Plugin 函数调用（不含目标资源耗时）p95 ≤ 50 毫秒；冷启动（无编译缓存）p95 ≤ 300 毫秒。**度量定义**（CHK226）：**命中** = acquire 容量成功且找到 sha256 匹配的已验证 `CompiledPlugin`，随后为本次调用创建 fresh Store/Instance；**冷启动** = 没有该 Plugin 的有效编译缓存并触发 Wasmtime 编译。压测脚本先调用同一 Plugin 完成编译 warmup，然后测量后续 100 次 fresh Store/Instance 调用的 p95。
- **SC-006**: Agent 路由决策 p95 ≤ 1.5 秒（含一次 LLM 决策调用）。
- **SC-007**: 越权 Capability 调用 100% 被宿主拒绝并审计，无任何漏放。
- **SC-008**: `main` Agent 删除尝试 100% 被拒绝。
- **SC-009**: Plugin 软删除前的引用检查 100% 准确（被引用时 0% 误删）。**Race window 规避**：软删除 transaction 必须 `SELECT ... FOR UPDATE` 锁住目标 Plugin 行，并在同一事务执行 `SELECT COUNT(*) FROM functions WHERE plugin_id = ?`；Function 是硬删除模型，不得引用不存在的 `functions.deleted_at`。并发新建 custom Function 必须在自己的 transaction 中 `SELECT id, deleted_at FROM plugins WHERE id = ? FOR SHARE`，确认未软删除后持锁完成 Function/tagging INSERT 与 COMMIT。创建先提交时删除随后看到引用并返回 4093；删除先提交时创建随后看到 `deleted_at` 并返回 4093，不允许两者同时成功。
- **SC-010（历史，已 superseded）**: 原管理端测试聊天曾要求完整对话端到端 p95 ≤ 8 秒；该 API/UI 删除后不再是 004 现行验收项。

## Threat Model

004 引入 WASM Plugin runtime + 多模型 LLM 调用 + 多级 Agent，扩展了 003 的攻击面。本节列出本特性显式纳入考虑的威胁与对应缓解 ——**未列出的威胁视为 out of scope 或继承自 003**。

### TM-1 恶意 / 错误 Plugin（assume hostile plugin author）

**威胁**：Plugin 作者上传含恶意逻辑的 WASM —— 试图访问宿主资源、发起 SSRF、读取其它 Plugin 的状态、绕过 capability。
**缓解**：
- WASM 默认无 WASI 系统调用 → 只能通过显式注册的 host function（FR-002）
- Capability dispatcher 在 host_call 入口零信任鉴权 + 拒绝路径强制审计（FR-003 / FR-004）
- Instance Pool 只缓存 `CompiledPlugin`；每次调用创建 fresh Store/Instance，linear memory、mutable global、table 均不跨调用（FR-029）
- `network.http` 强制 SSRF 防护（FR-001 v6）
- Plugin 上传期 import 静态检查（FR-005 v6）
- WASM 加载前 sha256 重新校验（FR-029 v6）

### TM-2 越权管理员（assume System role 操作恶意 / 失误）

**威胁**：System 角色试图给 Agent 授予危险 capability、删除 main 或修改 main 的 system_prompt。
**缓解**：
- 危险 capability 的赋予 / 撤销 = Super only（FR-022 v6）；System 在 UI 上看不到（hidden 而非 disabled）+ 后端再校验
- `main` Agent 删除拒绝（FR-022）+ 修改限 Super
- 所有变更走 audit log（FR-004）

### TM-3 凭据 / 密钥泄漏

**威胁**：JWT_SECRET / LLM API KEY / DB 密码 / `secret.get` capability 泄漏。
**缓解**：
- 沿用 003：env-injected，CI scan，DB URL 日志遮码
- `secret.get` allowlist（contracts/host-functions.md §4.6）+ 调用审计
- 错误响应不携带 stack trace（FR-025 v6 子 Agent 错误脱敏；通用 capability handler 同此原则）
- audit payload 由 capability-specific allowlist 生成，序列化后不超过 1 KiB；未知 capability 不记录 payload，正文、凭据、参数值和原始下游错误均不得进入日志

### TM-4 DoS / 资源耗尽

**威胁**：Plugin 死循环 / OOM；高并发刷 host_call；超大 WASM 文件。
**缓解**：
- Plugin 单次 timeout 30 s（FR-030）+ memory 128 MiB（FR-032）+ fuel-based 中断
- Pool 大小 + 等待超时（research §2）
- Plugin 文件 ≤ 16 MB（spec §Edge Cases）
- dispatcher 强制 `network.http` 8/Plugin/会话并发与
  `log.emit` 100/s/Plugin 令牌桶；限流拒绝仍逐次写安全 audit

### TM-5 数据外泄（含 LLM 提示词注入）

**威胁**：用户构造提示词让 LLM 调用越权 tool；子 Agent 错误回流给用户暴露内部 prompt；audit 日志被读出敏感字段。
**缓解**：
- Capability 鉴权对 LLM 决策不可逆（不论 LLM 输出什么，host_call 进入 dispatcher 仍按 Agent permissions 判定）
- 子 Agent 错误脱敏上报（FR-025 v6）
- runtime audit log 不可篡改（append-only）+ 默认 36500 天（100 年，可配置）后归档；非 Super 不可读取受限审计数据（003 权限模型）

### Out of Scope（继承自 003 或工程基础保障，不在 004 spec 重复约定）

- 中间人 / 网络层加密（依赖 TLS / VPC）
- 物理 / 主机层入侵（依赖基础设施安全）
- Wasmtime / Extism / providers crate 本身的 0-day（依赖上游修复 + 依赖扫描 CI）
- 管理员 PC 被入侵导致 JWT 泄露（依赖端点安全）

## Implementation Clarifications & Design Decisions

> 本节收纳 checklist 交叉核对中发现的"需显式记录的设计决策/接受的局限/补充约定"。

### REST API 补充约定（CHK183/185/186/187/188/192/203/204）

- **CHK183**: 10 组资源 CRUD 响应形态已在 contracts/api.md 各节给出示例；POST 创建端点统一返回 envelope 内完整对象（含 server-assigned `id`/`created_at`/`updated_at`）。
- **CHK185**: POST 创建端点返回的 `id` 与对象快照一致；server-assigned 字段由 DB 生成，client 不得在请求体中提供。
- **CHK186**: 所有 PUT 端点统一走乐观锁（请求体必含 client 读到的 `updated_at`），冲突→409+4094。此约定在 contracts §0+§Errors 固化。
- **CHK187**: DELETE 被引用阻塞→4093 统一适用于：Plugin（被 Function 引用）、Function（被 workflow_nodes/tools 引用）、Workflow（被 tools 引用）、Agent（有子 Agent）。Tag 用 4091。
- **CHK188**: multipart POST `/api/plugins` 字段顺序：`meta` 先于 `file`。服务端先读 meta 校验大小，再读 file。
- **CHK192**: 004 错误码（4030/4045/4091..4094/5001..5009/4291）与 003（1001..3004）命名空间无冲突：003 用 1xxx/2xxx/3xxx，004 用 4xxx/5xxx。
- **CHK203**: HTTP 状态码是 transport-level 映射，业务码（`code`字段）才是权威。前端必须按 `code` 分支。
- **CHK204**: Plugin 上传超过 `PLUGIN_MAX_BYTES`（默认16MB）时返 4001，`params: { "limit": "16MB" }`。

### Legacy 管理端 SSE Chat 补充（CHK193，已 Superseded）

- **CHK193（历史）**: 原 SSE payload schema 仅记录被移除的管理端测试聊天设计，不是现行 API 契约；不得据此恢复 admin chat。

### host_call ABI 补充（CHK198/207/209）

- **CHK198**: host_call request envelope 编码为 UTF-8 JSON，不含版本字段（v1 隐式；破坏性变更进 v2+双版兼容，host-functions.md §7）。
- **CHK207**: host_call payload 超 4MB 时返 `{ ok: false, code: 4001, message: "Payload exceeds 4MB limit" }`。
- **CHK209**: host_call ABI 版本契约已在 host-functions.md §7 明示：当前 v1；新增 capability 仅 §4 增项；envelope/错误码语义变更进 v2。

### Capability Coverage Matrix 补充（CHK201/202）

- **CHK201**: Matrix 覆盖现行管理端资源端点（Capabilities/Categories/Tags/Plugins/Functions/Workflows/Tools/Skills/Agents + Runtime Metrics）；原 Chat 组已 superseded。
- **CHK202**: 危险 capability 涉及的 Agent permissions 修改路径标记为 Super-only（与 FR-022 一致）。

### WASM 资源限制补充（CHK216/217/218/219）

- **CHK216**: 128 MiB linear memory 上限由 Extism manifest 的 `memory.max_pages=2048` 强制；换算公式固定为 `PLUGIN_CALL_MAX_MEMORY_MB × 16` 个 64 KiB page。超过上限的 grow/allocation 在运行时 trap，返回 5000 + 安全 tracing + bounded best-effort DB runtime audit，并丢弃本次 fresh Store/Instance。
- **CHK217**: Wasmtime 默认调用栈深度上限 10000 帧（`wasmtime::Config::max_wasm_stack`）。恶意递归触发 stack overflow trap → 丢弃本次 fresh Store/Instance + audit。
- **CHK218**: host_call payload 4MB+network.http body 4MB 是 spec-level 硬约束（host-functions.md §5），非 implementation detail。
- **CHK219**: 各 capability 并发上限语义：单宿主进程内
  `network.http` 8/Plugin/会话，key 为 `(plugin_id, Option<session_id>)`，
  `None` 是共享 sentinel；`db.query`/`db.execute` 受 sqlx 连接池；
  `llm.invoke` per-session 串行；`log.emit` 100/s/Plugin 令牌桶。

### 性能预算可测量性补充（CHK227/229）

- **CHK227**: SC-006 基准使用 mock LLM 固定 800ms；生产通过 tracing span `routing_ms`+`llm_ms` 分离度量。
- **CHK229**: perf 工具：T140 tokio Bencher; T156 criterion+mock; T168 criterion+mock LLM。命令记录于 perf-evidence.md。

### 并发与背压补充（CHK230/231/233）

- **CHK230（历史，已 superseded）**: 原管理端 SSE 全局/per-admin 并发限制随 Chat API 移除，不再是 004 运行时要求。
- **CHK231**: Instance Pool 的 `PLUGIN_POOL_MAX_PER_PLUGIN=8` 只限制并发 WASM
  invocation slot，不能替代 capability 限流。dispatcher 另行强制
  `network.http` 8/Plugin/会话与 `log.emit` 100/s/Plugin；当前语义为
  process-local，不承诺跨 HiveWeb replica 合计。
- **CHK233**: DB 连接池 `max_connections` 默认 20。audit 写入走同一池；运维可调 `SQLX_MAX_CONNECTIONS` env。

### 启动与生命周期补充（CHK236/237）

- **CHK236**: Graceful shutdown：SIGINT 或 SIGTERM 到达后立即停止 accept 新连接，等待活跃 HTTP 请求完成且最多 30 秒；随后停止并等待 idle reaper，drop Instance Pool/`CompiledPlugin` 缓存，最后关闭 DB pool。30 秒到期后取消剩余请求并继续清理，不得因 drain 无限阻塞退出。原“等待 admin SSE 并标记 session interrupted”语义已 superseded。
- **CHK237**: 在线 reload `llm_presets.toml`：MVP 不支持（YAGNI）。修改 preset 需重启 hiveweb。

### 观测性补充（CHK239/240）

- **CHK239**: Plugin 调用 tracing span 字段：`plugin_id`/`function_id`/`capability`/`outcome`/`elapsed_ms`/`pool_hit`+`request_id`。
- **CHK240**: LLM 调用 tracing 字段：`model_preset`/`actual_model`/`fallback_used`/`reason`/`llm_ms`+`request_id`。provider 切换用 `llm_fallback`；本地文本兜底用 `llm_local_fallback`，不得混记。

### 安全补充（CHK059..122 全覆盖）

- **CHK059**: 不存在"未经 dispatcher 的 host_call 捷径"：WASM sandbox→host function 唯一入口→host_call→dispatcher。三层保证无旁路。
- **CHK060**: Capability 注册表运行时不接受外部修改（hot reload）。注册表是编译期常量+启动期 DB upsert 镜像。
- **CHK061**: `is_dangerous=1` 的 capability 列表在实际 V014 seed 显式列出：`network.http`/`db.execute`/`secret.get`。修改需新 migration+Super 审批。
- **CHK062**: 危险 capability 赋予要求 Super（FR-022）；调用由 dispatcher 按当前 Agent.permissions 鉴权。两者一致。
- **CHK065**: 危险 capability 列表修改走 migration+DBA+Super 审批。MVP 不提供 UI 修改 `is_dangerous`。
- **CHK067**: named query 注册流程：`named_queries.toml` 配置文件，仅 Super 可修改+重启服务。MVP 不提供 API 注册。
- **CHK068**: named query 入参绑定使用 sqlx 参数化占位符（`?`/`:name`），永不字符串拼接。
- **CHK069**: named query 含危险 SQL 的审批：需 Code Review+DBA 审批后部署。
- **CHK070**: DB 连接分账户：MVP 不分。安全隔离依赖 capability 鉴权+named query 参数化+行数限制。
- **CHK071**: `secret.get` allowlist 在宿主配置文件中定义。请求不在 allowlist 的 key→4030 denied。
- **CHK072**: secret enumerate 明确禁止。args schema 仅接受 `{ "key": string }`。
- **CHK073**: secret 访问审计由 FR-004 覆盖：每次 `secret.get` 写 audit。
- **CHK074**: secret 在内存中不缓存到 static/thread-local；日志中不输出 secret key/value 原文，仅允许记录 key 的稳定短指纹。
- **CHK075**: `network.http` 白名单域名来源：全局配置（`http_allowlist`），per-Agent 可收窄（MVP 不加 per-Agent domain 字段）。
- **CHK077**: 出站请求代理隔离：MVP 不要求，走直连+SSRF 防护。
- **CHK078**: network.http body 4MB+并发 8/Plugin 是 spec-level 硬约束。
- **CHK079**: WASM 默认无 WASI 系统调用（research §11+TM-1 已明确）。Extism
  构建器显式使用 `with_wasi(false)`；真实 WASM 回归必须同时证明 WASI import
  实例化失败且 `extism:host/user.host_call` 仍可执行。
- **CHK082**: linear memory 128MB+调用栈深度+fuel timeout 是 spec-level 硬约束（FR-029/030/031）。
- **CHK084**: Plugin 调用之间不共享状态：只复用 `CompiledPlugin`，每次调用 fresh Store/Instance，因此 linear memory、mutable global 与 table 天然隔离。
- **CHK085**: 跨 Agent 复用 `CompiledPlugin` 时，dispatcher 仍使用当前调用 Agent 的 permissions；Store/Instance 不跨 Agent 复用。FR-003 / FR-029 已明确。
- **CHK086**: Plugin 实例创建期间 panic→丢弃实例+audit+返回 5000。
- **CHK087**: Pool 等待期间 permission 变更：已发出的 host_call 用入口时 permissions 快照。TOCTOU 接受窗口（≤30s）。
- **CHK089**: `payload_summary` 必须由 capability-specific allowlist 构造并保证序列化后 ≤1 KiB，禁止依赖通用字段黑名单。`network.http` 仅保留 method、成功校验后的 host、body 字节数和 header 数；文件/S3 仅保留路径或 key 指纹与字节数；DB/LLM/secret/log 仅保留计数、长度或指纹。未知 capability 的 summary 必须为 `None`。URL path/query、header 值、正文、Prompt、DB 参数、secret、Plugin 日志正文/字段名/字段值及原始下游错误不得进入 tracing 或持久化审计。
- **CHK090**: audit 仅 INSERT，不 UPDATE/DELETE（除超期清理）。MySQL 用户仅授 INSERT+SELECT。
- **CHK091**: `AUDIT_RETENTION_DAYS` 默认 36500 天（100 年，可由用户调整）；保留期满→归档到 S3（JSON Lines 按月分桶）+删除原表行。MVP 可简化为直接删除。
- **CHK091A**: 所有非 Hook runtime audit 必须先写安全 tracing，再通过单 worker 有界队列 best-effort 持久化；队列满、writer 未安装或 DB 失败只丢失持久化副本，不阻塞 Agent 主流程，且日志只记录静态错误类型。Capability permission DB 失败及 Plugin lookup/pool/S3/SHA/compile 等提前返回也必须 exactly-once 进入同一安全审计收口。Hook 执行保持 tracing-only。
- **CHK092**: `request_id` 跨边界传递：axum request-id middleware→Context→dispatcher/pool/orchestrator。所有 audit 行共享同一 request_id。
- **CHK093**: main Agent `system_prompt` 修改限 Super（FR-022 "任何字段修改仅 Super"已含）。
- **CHK094**: main Agent `model_preset` 允许为空（=全局默认）。全局默认 preset 变更属部署级操作。
- **CHK095**: 禁止创建 identifier='main' 的 Agent — UNIQUE+service 层额外校验。
- **CHK096**: Super 降级恢复：沿用 003 设计（至少保留 1 个 Super）。
- **CHK097**: Agent 嵌套≤10 的理由：①防指数级 LLM 调用；②防上下文窗口溢出；③运营经验值。
- **CHK098**: 路由循环检测窗口：同一次 Agent 运行范围。orchestrator 维护 agent visit history；不依赖已移除的 admin chat session。
- **CHK101**: 所有 capability 入参在 dispatcher 层做 JSON schema 校验，非法→4001+audit。
- **CHK102**: identifier 字符集：`[a-z0-9_]+(\.[a-z0-9_]+)*`，2-64字符。service 层正则校验。
- **CHK103**: WASM magic bytes 校验在 FR-005 已明确。
- **CHK104**: multipart upload 校验顺序：先读 meta→校验元数据→再读 file→校验 magic+大小→计算 sha256。
- **CHK105**: 004 所有新增端点走 003 JWT。无遗漏。
- **CHK106**: `GET /api/capabilities` 和 `GET /api/agents/model-presets` Normal 可访问。
- **CHK108**: Plugin S3 key：`plugins/{identifier}/{version}.wasm`。sha256 不嵌入 key。
- **CHK109**: schema_migrations 走 sqlx 内置互斥锁，不走乐观锁。
- **CHK110**: Plugin 软删除后重建同 identifier+version：service 层拒绝。
- **CHK111**: Agent permissions 修改与正在执行的 host_call：已发出用入口时快照，后续用新。TOCTOU 窗口≤30s。
- **CHK113**: capability handler 内部抛错→统一包装为 5000+用户文案；只有宿主内部的 typed/static `InvalidArguments` / `Timeout` 分类可分别改变为 4001 / 4081。provider content、下游错误正文或其中出现的 `timeout` 等子串不得决定 HTTP reply code 或 audit 分类。reply、tracing 顶层 `error_kind` 与 capability-specific allowlist 摘要必须复用同一个安全分类。原始 provider/DB/S3/Extism 错误、URL/payload、凭据和 stack trace 不得进入 audit/tracing。
- **CHK114**: Plugin 反复 4030 的 fail-fast：MVP 不自动熔断。依赖 rate limit+audit 告警。
- **CHK115**: 失败 host_call 返回调用方时，`message` 使用用户文案，不暴露内部细节；原管理端 SSE `error` 载体已 superseded。
- **CHK117**: audit append-only 语义（CHK090）。hash chain：MVP 不加（YAGNI）。
- **CHK118**: JWT_SECRET/LLM API KEY 存储：env-injected 不落盘。轮换=更新 env+重启。
- **CHK119**: DB 连接字符串密码在日志中遮码：沿用 003 `Secret` wrapper。
- **CHK120（历史，已 superseded）**: 原 admin chat 的 PII/30 天清理设计已移除；现行用户聊天的数据保留策略不由 004 定义。
- **CHK121**: 审计日志合规导出：MVP 不提供 API。运维直接查 DB。
- **CHK122**: LLM 调用"不发送敏感字段"过滤：MVP 不实现。缓解：payload_summary 脱敏+子 Agent 错误脱敏。

### UX 补充（CHK001..054 全覆盖）

- **CHK001**: Plugin 上传进度：MVP 显示 spinner+"上传中…"，不提供实时进度条。
- **CHK002**: 大文件上传失败显示 4001+"Plugin 文件大小超过 16MB 限制"。
- **CHK005/CHK006（历史，已 superseded）**: 原管理端 SSE `routed` / `tool_call` 渲染不再是现行 UI 验收条件。
- **CHK007**: DAG 编辑器空状态：提示"拖入 Function 开始构建工作流"+空画布。
- **CHK008**: 节点级失败 UI：节点高亮红色+tooltip 显示错误信息。
- **CHK009**: Skill frontmatter 校验时机：保存时校验（非实时）。
- **CHK010**: ModelPreset 下拉为空：显示"未找到可用模型配置，请联系管理员"。此场景不应发生。
- **CHK011**: 拖拽 Function：reactflow 标准拖拽。从左侧列表拖入画布创建节点。
- **CHK012**: DAG 边端口映射：从 output port 拖到 input port+边属性面板。
- **CHK013**: 环检测红框：仅环上节点+连线高亮红色。
- **CHK014（历史，已 superseded）**: 原管理端 SSE `done` 事件结构不再是现行 API 契约。
- **CHK016**: `system_prompt` 最大长度 32KB，超出→4001。textarea 显示字符计数器。
- **CHK017**: Skill content 64KB 超出→保存时拒绝+toast 提示。
- **CHK018**: Agent 编辑器字段顺序与 contracts §9 一致。
- **CHK019**: "无权限=不渲染"在 004 全部 5 个新页面贯彻。
- **CHK021**: Plugin 列表"已删除"行灰显+日期标签，与 003 "已禁用"视觉统一。
- **CHK022（历史，已 superseded）**: 原管理端 SSE chat 视觉要求已移除。
- **CHK023**: SC-001 度量起点=HTTP 请求到达，终点=200/201 响应。
- **CHK024**: SC-003 含服务端校验耗时，不含 reactflow 渲染。
- **CHK025**: SC-006 含 LLM 调用延迟。宿主侧 routing overhead≤200ms。
- **CHK026（历史，已 superseded）**: 原管理端 SSE 首字/`done` 度量已移除。
- **CHK027**: a11y 覆盖全部 004 新增页面+组件。reactflow canvas 可豁免部分 axe 规则。
- **CHK028**: Plugin 软删除后 Function 下拉过滤已删除项。
- **CHK029**: Workflow 引用已删除 Plugin 的 Function 节点：灰色+"Plugin 已删除"标签。
- **CHK030**: model_preset 不存在时保存返 5007。
- **CHK031（历史，已 superseded）**: 原管理端聊天历史虚拟滚动要求已移除。
- **CHK032**: Skill 勾选后右侧面板实时预览 system_prompt 拼接结果。
- **CHK033**: 同 identifier 不同 version Plugin：列表行显示 version badge。
- **CHK034**: Agent 第 10 层新建子 Agent 按钮隐藏（不渲染）。
- **CHK035**: main Agent 在树中锚定：不可拖动、无删除按钮、显示 `main` badge。
- **CHK036/CHK037（历史，已 superseded）**: 原管理端 SSE error 与 Chat 提示要求已移除。
- **CHK038**: DagEditor keyboard navigation：Tab/Enter/Delete/Esc。
- **CHK039**: Monaco editor a11y 限制：axe 排除 Monaco 容器，手动验证。
- **CHK040**: reactflow jsdom 限制：axe 排除容器；a11y 通过 Playwright E2E 验证。
- **CHK041（历史，已 superseded）**: 原管理端 SSE 浏览器兼容性要求已移除。
- **CHK042**: i18n：沿用 003 默认中文。
- **CHK043（历史，已 superseded）**: 原管理端 Chat 长内容渲染要求已移除。
- **CHK044**: reactflow 11.x 稳定性：锁定 minor 版本。
- **CHK045**: Monaco worker 加载失败→降级 textarea。
- **CHK046**: 假设管理员熟悉 markdown+JSON Schema：显式登记。
- **CHK047（历史，已 superseded）**: 原管理端 EventSource 假设已移除。
- **CHK049（历史，已 superseded）**: `route_to_subagent` 仍是运行时特殊 Tool，但原 SSE `tool_call` + `routed` 客户端表现不再是现行契约。
- **CHK050**: Tool 与 Skill 在 Agent 编辑器多选框中区分视觉：不同 section+不同 icon。
- **CHK051**: SkillsLoader frontmatter 解析失败→UI toast+保存拒绝。
- **CHK052**: 内置/定制 Tool 视觉区分：`builtin`/`custom` badge。
- **CHK053（历史，已 superseded）**: FallbackProvider 仍可切换后端，但原管理端 SSE `fallback_used` UI 要求已移除。
- **CHK054**: LlmPresetName 命名约束：`[a-z0-9_-]+`，2-64字符。UI 下拉按字母排序。

### 需求完整性/清晰度补充（CHK002..072）

- **CHK002/003/004/006**: 已在 CHK193/内置 Function schema/FR-005/安全门错误码中补充。
- **CHK015**: Plugin 内存 128 MiB 超限 → Wasmtime trap → 5000 + 安全 tracing + bounded best-effort DB runtime audit + 丢弃本次 fresh Store/Instance；5004 仅表示 Plugin timeout。
- **CHK017**: 跳次计数：main=第0跳，main→子=第1跳。`AGENT_MAX_HOPS` 限制路由次数。
- **CHK021/CHK023（历史，已 superseded）**: 原 admin chat 保留 cron 与七类 SSE 事件要求随管理端测试聊天移除。
- **CHK029**: SC-001 度量起点=HTTP 请求到达，终点=200/201 响应。
- **CHK032**: SC-007 测试方法：11 capability×3 路径=33 个合约测试用例。
- **CHK054/057/058/059**: 度量工具/a11y 验收/错误不携带 stack trace/16MB 双重校验已补充。
- **CHK061/062/063**: providers 复用/003 角色复用/S3 兼容性验证已在 tasks 中体现。
- **CHK065**: DagEditor keyboard navigation 已排期（T157-T161）。
- **CHK067（历史，已 superseded）**: 原管理端“正在思考”客户端超时已移除；LLM/runtime timeout 仍按各自现行配置处理。
- **CHK068**: named query schema 在 `named_queries.toml`：`{ name, sql, params_schema }`。
- **CHK069**: Skill 模板插值：`{{function:identifier(args)}}`。MVP 仅支持此语法。
- **CHK070**: model_preset 切换不影响已开始的 Agent 运行（使用启动时解析结果）；不依赖已移除的 admin chat session。
- **CHK071**: Workflow 失败无回滚/补偿。DAG 执行"尽力向前"。
- **CHK072**: Pool lazy 初始化与冷启动≤300ms：首访可能 100-500ms，SC-05 已区分命中/冷启动。

### Data Model 补充（CHK128..182）

- **CHK128**: `agents.identifier VARCHAR(64)` 足够（最长合理路径约 40 字符）。
- **CHK129**: `model_preset` 字符集 `[a-z0-9_-]+`，2-64字符。
- **CHK130**: `payload_summary` 采用 CHK089 的 capability-specific allowlist，序列化后硬限制 ≤1 KiB；超限时只保留 `ok`、静态 `error_kind` 与 `summary_omitted=size_limit`。
- **CHK131（历史，已 superseded）**: 原 `chat_messages` admin 表字段设计已移除；现行用户表为 `chat_messages_user`，其字段约束由外部 Assistant API 与迁移负责。
- **CHK132**: `workflow_edges.mapping` schema：`{ "dst_key.dst_field": "src_key.src_field" }`。
- **CHK135**: `agents.identifier` 全局唯一，不以 parent 前缀区分。by-design。
- **CHK136**: `categories (parent_id, slug) UNIQUE` 允许跨 parent 同 slug。by-design。
- **CHK137（历史，已 superseded）**: 原 admin `chat_messages (session_id, seq)` 设计已移除；现行 `chat_messages_user` 没有该 `seq` 契约。
- **CHK138**: FK ON DELETE 理由：CASCADE=强拥有；SET NULL=弱引用+需保留；RESTRICT=引用阻塞；无 FK=审计不可级联。
- **CHK143**: `runtime_audit_logs` 无 FK→intentional（audit 不可级联删除）。
- **CHK144**: 不变量每条标明"谁强制"：1/2/8=service+FE; 3=DB CHECK+service; 4/6/7/10/11/12=service; 5/9/13=DB。
- **CHK149**: 三维检索索引足够。EXPLAIN 验证在 T049。
- **CHK150**: "按 agent_id 统计"用现有索引，性能不足时加复合索引。
- **CHK153**: 长期增长表分区：MVP 先不做（<1M 行），3 个月后按需加。
- **CHK159**: forward-only migration（沿用 003）。
- **CHK162**: `is_dangerous` false→true 时，已授权 Agent 不自动收回。需 Super 手动审计。
- **CHK164**: `skills.frontmatter` 允许的 key 子集：MVP 不限制。
- **CHK165**: 内置 Skill seed 与 builtin Function 一致—启动期代码 upsert。
- **CHK167**: `plugins.manifest` 仍存储完整 manifest，运行时忽略 allowed_hosts/allowed_paths。
- **CHK168**: `plugins.s3_key` 格式：`plugins/{identifier}/{version}.wasm`。sha256 不嵌入 key。
- **CHK169**: Plugin 软删除后 sha256/s3_key/S3 文件保留供 audit 追溯。
- **CHK174**: `LlmPresetName`/`model_preset` 三处命名一致。
- **CHK177**: SC-005 "命中"定义在 spec SC-005 度量定义段，data-model 无需重复。
- **CHK178**: SC-007 可测量方法：33 个合约测试+每月人工抽检。
- **CHK180**: MySQL ≥8.0.4 假设显式登记。
- **CHK182**: sqlx compile-time 对动态 query 的限制：集成测试覆盖所有 8 种组合。

## Assumptions

- LLM 调用一律走 workspace 中既有的 `crates/providers`（多 backend 抽象 + Fallback + tool-calling）。Agent Runtime 不直接持 vendor SDK，只通过 `LLMProvider` trait 调用。
- LLM 失败回退由 `providers::FallbackProvider` 链承担；启动配置以 preset 形式给出 primary + fallback 列表（如 `primary=openai-compat, fallback=[bedrock, anthropic]`）。registry 在启动期构造并缓存每个命名 preset 的完整 provider chain；所有真实 runtime LLM 调用点必须经 `LlmRegistry::build_chain` 克隆缓存的 `Arc`，单 provider preset 可返回 primary，多 provider preset 必须返回 primary + `providers[1..]`，不得逐次构造 provider、调用/保留 `build_primary` 或退化为 primary-only。fallback 条目以 preset + ordinal 稳定寻址，不以 model 反查。整条链失败时由现行调用方 API 返回错误；004 不再规定管理端 SSE `error`。
- 同时支持多个命名 preset（如 `cheap-fast` / `code-expert` / `claude-fallback`），每个 Agent 通过 `model_preset` 字段选择；只有 NULL 使用全局 default，显式未知值 fail closed。
- 已有的管理中心（003-admin-center）提供的角色与认证体系直接复用；Runtime 端点保护沿用同一 JWT。
- 对象存储沿用既有 Rustfs（S3 兼容）部署，不引入新存储。
- Plugin 作者用 Extism 官方 SDK（Rust/Go/JS/Python 等任一）；宿主仅承诺 Extism PDK 兼容。
- DAG 编辑器使用前端通用图编辑库（具体选型在 plan）。
- 内置 Function 由系统启动时静态注册；不支持热加载内置函数。
- 原 admin chat 的 30 天保留假设已 superseded；004 不定义现行用户聊天的保留期。
- Capability `db.execute` / `db.query` 仅允许宿主预注册的命名查询/写入（zero-trust），自由 SQL 永不暴露 — 已锁定。
- `is_always = 1` 的 Tool/Skill 对所有 Agent 自动可用/加载，无需在 `agent_tools` / `agent_skills` 中建立关联。
- `source = 'builtin'` 的 Tool 不可编辑，只能包装 builtin Function (kind=1)。

## Implementation Status

> 004-agent-runtime 保留 237 个历史任务，并增加 T238–T240 收敛任务。当前
> 可执行迁移链以物理 SQL 与 `migrate.rs::MIGRATIONS` 为准：**V001–V033**，
> **V013 为保留空号**。

### 实际数据库迁移边界

- **V005–V012**：Agent Runtime 核心基础表。旧设计中曾分配到
  “V019–V038”的 Tool/Skill/Function/Workflow/Capability 后补字段，已经
  合并进这些基础迁移。
- **V014**：main Agent 与 Capability seed；**V016**：Capability 分类 seed。
- **V015、V017–V031**：推荐游戏、普通用户聊天、全局配置、游戏/别名、
  Agent Hook、敏感词及其后续演进，详见 `data-model.md` 的物理清单。
- **V032**：非 Hook `runtime_audit_logs`。
- **V033**：将 `workflows.timeout_ms` DB 默认值规范化为 33000 ms。
- 历史“V019–V038 Agent Runtime 扩展”仅作为已合并的规划记录保留；当前
  V019–V031 已被其它物理迁移占用，不得按旧设计新建或重命名 SQL，也
  不存在物理 V034–V038。

### 新增功能（不在原始 spec 中）
- **Admin Center**（继承 003）：Admin CRUD / LoginRecord / AdminAuditLog / RBAC / Super Admin 保护
- **Dashboard**：统计面板（各类资源计数 + 最近活动 + Plugin Pool 健康度）
- **RecommendedGame**：推荐游戏 CRUD + 排序管理
- **Capability CRUD**：capability 管理 + category 关联
- **RuntimeAuditLog**：运行时审计日志查询
- **LoginPage**：管理员登录页面
- **通用组件**：Layout / PermissionGuard / FunctionTester / ToolTestModal / SkillTestModal / AdminForm / api.ts / auth.ts
- **Bin 工具**：seed / seed_bench / create_super_admin
- **前端分页**：TagPage 分页

## Outstanding Clarifications

✅ 全部 12 项已通过 4 轮 `/speckit-clarify` 收敛，决议记录于 §Clarifications。本节保留为历史索引：

| # | 主题 | 决议 |
|---|------|------|
| 1 | LLM 提供商 | 复用 `crates/providers` 多 backend 抽象（含 Anthropic / OpenAI 兼容 / Bedrock / Codex / Copilot / Azure / Fallback） |
| 2 | Agent 路由决策 | LLM tool-calling 自决 + hard-rule（深度 ≤ 10 / 循环 / 越权拒绝） |
| 3 | Plugin 多版本 | 共存；唯一 `(identifier, version)`；Function 绑定具体版本主键 |
| 4 | 聊天历史保留 | 历史决议，已随 admin chat superseded；现行用户聊天保留期不由 004 定义 |
| 5 | 路由跳次上限 | 5（默认，可配 `AGENT_MAX_HOPS` env） |
| 6 | Plugin 文件大小 | 16 MB（默认，可配 `PLUGIN_MAX_BYTES` env） |
| 7 | `db.execute` 范围 | 仅宿主预注册的命名查询/写入；自由 SQL 永不暴露 |
| 8 | 流式回复 | 历史决议，已 superseded；不得恢复管理端 SSE Chat |
| 9 | Plugin 调用 timeout / memory | 30 秒 / 128 MB（默认，可配 env） |
| 10 | 并发编辑冲突 | 乐观锁 `updated_at`；冲突返回 409 |
| 11 | 子 Agent capability 边界 | 不继承；按当前执行中的 Agent 鉴权 |
| 12 | Instance Pool 跨会话复用 | 仅 `CompiledPlugin` 可复用；Store/Instance 及 memory/global/table 不复用 |
| 13 | Tool vs Skill | Tool 走 OpenAI tool-calling；Skill 拼 system_prompt |
| 14 | Per-Agent LLM 选型 | `model_preset` 字段；只有 NULL → 全局默认，显式未知值 fail closed |
| 15 | LLM 失败回退 | 启动期缓存完整 FallbackProvider 链；稳定 provider identity 不依赖 model；25s/node、45s/chain，`llm.invoke` 25s |
| 16 | Agent CRUD 权限 | main 编辑限 Super；危险 capability 赋予限 Super |
| 17 | 内置 function 集合 | `format_template` / `json_parse` / `json_stringify` / `text_regex_match` / `chat_respond` |
