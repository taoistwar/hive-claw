# Feature Specification: Agent Runtime（基于 Capability 的 WASM 插件 Agent 运行时）

**Feature Branch**: `004-agent-runtime`
**Created**: 2026-05-26
**Status**: Draft
**Input**: User description: 添加 Agent Runtime（Capability-based with Extism WASM Plugin），它包括能力管理、分类管理、标签管理、WASM 插件管理、函数管理、Workflow 管理、工具管理、技能管理、Agent 管理、测试聊天窗口。

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
- Q: 聊天端点是否必须流式返回？ → A: 强制 SSE（text/event-stream）；client 即便不读流也能在 `done` 事件拿到最终结果
- Q: 同 Plugin identifier 多版本如何处理？ → A: 多版本共存；唯一性 = `(identifier, version)`；Function.plugin_id 绑定具体版本主键；升级 = 新版本 + 新 Function 显式接入
- Q: LLM 客户端/供应商抽象如何选？ → A: 复用 workspace 已有的 `crates/providers`（提供 `LLMProvider` trait + Anthropic/Azure-OpenAI/Bedrock/OpenAI-Compat/Codex/GitHub-Copilot/Fallback 多 backend + ProviderRegistry + ToolCallRequest）。Agent Runtime 不引入 async-openai，新增的 `runtime/llm.rs` 仅作 thin adapter：通过 `providers::make_provider(&cfg)` 拿到 `Arc<dyn LLMProvider>` 后转发 chat / tool-calling。
- Q: LLM 调用失败时的回退策略？ → A: 强制使用 `providers::FallbackProvider` 链；启动期按 preset（`primary` + `fallback: [...]`）构造一次，runtime 不感知具体 backend；链全部失败才作为 SSE `error` 事件抛出。
- Q: Agent CRUD 权限矩阵？ → A: `main` 任何字段修改限 Super；非 main Agent CRUD = System+；不论哪个 Agent，permissions 含 `is_dangerous=1` 的 capability（`network.http` / `db.execute` / `secret.get`）必须 Super 提交。
- Q: MVP 内置 function 初始集合？ → A: 启动期注册 5 个胶水函数：`format.template`（模板填充）/ `json.parse` / `json.stringify` / `text.regex_match` / `chat.respond`（生成用户可见回复）。这些不依赖 WASM、不可删除、可被禁用。
- Q: 路由到子 Agent 后 host_call 鉴权看哪个 permission 集合？ → A: 当前执行中的 Agent 自己的 permissions；不继承父 Agent，也不取交集。Capability 始终按"最近的 Agent 框架"判断，符合最小权限 / zero-trust。
- Q: Instance Pool 中的 Plugin 实例是否允许跨会话 / 跨 Agent 复用？ → A: 是。Plugin 实例在归还前强制 reset linear memory，可跨 Agent / Session 自由复用。Plugin 作者**不得**依赖 mut global 跨调用持久化状态；PDK 文档进一步声明该约束。
- Q: Skill 与 Tool 在 LLM 看到的工具列表中如何区分？ → A: 沿用已有 `crates/agent` 模式。Tool 注册到 `agent::ToolRegistry`，LLM 通过 OpenAI tool-calling 看到（含 JSON Schema 入参/出参）；Skill 是 markdown 内容，调用 = `SkillsLoader` 把内容拼进当前 Agent 的 system prompt，**不**出现在 LLM tools 列表。Workflow 在 runtime 内被包装成一个 Tool 注册（Workflow Wrapper）。
- Q: 既然 `crates/agent` / `crates/skills` / `crates/providers` 已实现 agent loop / tool registry / skills loader / multi-provider LLM，004 应该复用哪些？ → A: 复用 `crates/agent::{AgentRunner, ToolRegistry, SubagentManager, SkillsLoader, ContextBuilder, MemoryStore}` 与 `crates/providers::{LLMProvider, FallbackProvider}`；004 在 hiveweb 中新增的是**管理面**（DB-backed 元数据 + HTTP CRUD + DAG 编辑器 + WASM Plugin Extism 适配器 + Capability dispatcher），**不**再造 agent 编排核心。
- Q: 不同 Agent 是否允许选不同的 LLM 模型 / FallbackProvider 链？ → A: 是。`agents` 表加 `model_preset VARCHAR(64) NULL` 字段，对应 hiveweb 启动时从 `llm_presets.toml` 加载的命名 preset（每个 preset 内部封装一个 `providers::FallbackProvider`：primary + fallback 链）。为空 = 走全局默认 preset（启动校验必须正好 1 个 `default = true`）。子 Agent 不继承父的 preset。
- Q: SSE 事件顺序 / 加载态 / 中途断开 UX 如何定义？（CHK003/004/048）→ A: 事件类型 token/tool_call/tool_result/routed/done/error；`done` 必有且仅 1 次，与 `error` 互斥；提交后到首事件期间显示"正在思考…" + 30s 无响应超时；SSE 中途断开显示"连接中断 + 重新发送"按钮，user 消息已 persist，assistant 中断内容不写库。详见 FR-028 重写。
- Q: 危险 capability 在 CapabilityPicker 中对 System 角色的行为？（CHK015）→ A: 不渲染（hidden）而非 disabled。Super 才看得到；后端在保存时再校验一遍角色，前端绕过仍会被 2001 拒绝。详见 FR-022 补强。
- Q: 全部 14 个错误码的用户可见文案如何统一？（CHK020）→ A: contracts/api.md §Errors 用三列表（code / 内部含义 / 用户文案）固化全部 14 项；前端 `web/src/utils/error_messages.ts` 集中维护映射，组件只引该字典。
- Q: capability 鉴权失败的 audit 行为？（CHK088）→ A: 4030 / 4040 拒绝路径**必须**写 audit log（`event_type = capability_denied`）— FR-004 v7 已强化。
- Q: 危险 capability 撤销动作的角色门限？（CHK063）→ A: 与赋予一致，仅 Super 可执行（防 System 在 Super 不在场时绕审计降权后再改 system_prompt）— FR-022 v7。
- Q: network.http 的 SSRF 防护？（CHK076）→ A: dispatcher 解析 URL 后拒绝 IPv4/IPv6 私网段 + 云 metadata 端点；DNS 解析后重校验实际 IP（防 DNS rebinding）；仅放行 allowlist 域名（per-Agent 可收窄）— FR-001 v7。
- Q: WASM 上传期静态校验？（CHK080）→ A: magic bytes + 文件大小 + import 全在 host 注册表内 + 忽略 manifest 自提权字段 + sha256 计算 — FR-005 v7。
- Q: 子 Agent 错误如何向父上报？（CHK099）→ A: 仅暴露 `{code, message}` envelope；system_prompt / 内部 tool 名 / stack trace / plugin identifier 不外泄；详情写 audit — FR-025 v7。
- Q: route_to_subagent 是否允许跨级路由？（CHK100）→ A: 不允许；必须是当前 Agent 的直接子 Agent — FR-025 hard-rule 第 ④ 项。
- Q: SSE chat 端点的会话所有权？（CHK107）→ A: JWT.admin_id == session.admin_id；Super 例外但仍走显式判定 — FR-027 v7。
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

### User Story 6 - 测试聊天窗口 (Priority: P2)

管理员在 Web 上输入文字，后端从 `main` Agent 开始运行，返回最终回复（可流式）。

**Independent Test**：输入"你好" → 几秒内返回回复 → 输入触发路由的问题 → 看到（最终）由子 Agent 应答。

**Acceptance Scenarios**:
1. **Given** 管理员在测试窗口, **When** 输入文字并点击发送, **Then** 收到 `main` Agent 的回复。
2. **Given** 一段对话已开始, **When** 管理员继续输入, **Then** 同一会话内的历史可被 Agent 看到（多轮上下文）。
3. **Given** Agent 调用模型超时, **When** 用户等待, **Then** 后端在 30 秒内（默认 LLM 单次调用上限，可通过环境变量调）回退到 FallbackProvider 下一节点，或在整条链全部超时后通过 SSE `error` 事件回报。

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
- 流式回复中途断开：MVP 不保留中断点；client 收到断开后应重新发起请求。已写入的 user 消息保留在 `chat_messages`；assistant 中断的部分按未完成处理（不写库）。后续版本若需 resume，再在 ChatMessage 增 `is_partial` 字段。

## Requirements *(mandatory)*

### Functional Requirements

**Capability 系统**
- **FR-001**: 系统必须暴露一组宿主级 Capability，包括但不限于：`network.http`、`fs.read`、`fs.write`、`s3.read`、`s3.write`、`db.query`、`db.execute`、`llm.invoke`、`secret.get`、`time.now`、`log.emit`。
  **`network.http` SSRF 防护**（hard requirement）：dispatcher 在转发前必须解析 URL 并拒绝以下目标 — ① 私网段（IPv4 `10/8` `172.16/12` `192.168/16` `169.254/16` `127/8` `0.0.0.0`；IPv6 `::1` `fc00::/7` `fe80::/10`）；② 云元数据端点（`169.254.169.254`、`metadata.google.internal`、`metadata.azure.com` 等）；③ DNS 解析后**重新**校验解析出的实际 IP（防止 DNS rebinding）；④ 仅放行宿主配置的 allowlist 域名（per-Agent 可进一步收窄）。
- **FR-002**: 所有 Capability 调用必须经 `host_call(capability_name, payload_bytes) → result_bytes` 单一 ABI 入口。
- **FR-003**: 宿主必须按"当前正在执行的 Agent 的 permissions 集合"鉴权；不在集合内的 capability 立刻拒绝。子 Agent 不继承父 Agent 的 permissions，也不与父取交集 — 每个 Agent 的能力集合独立、显式声明（最小权限 / zero-trust）。
- **FR-004**: 每次 Capability 调用都必须写入结构化审计日志：发起 Agent、Tool、Function、Plugin、capability、入参摘要、出参摘要或错误、耗时。**鉴权拒绝路径同样必须写**：当 host_call 因 `code = 4030 Capability denied` 或 `4040 Capability unknown` 被拒绝时，dispatcher 在返回错误前**必须**写一条 audit log（`event_type = capability_denied` / `outcome = denied`，含拒绝原因 + 调用方完整上下文）。"成功路径写、失败路径不写"是不可接受的 — 失败路径恰恰是安全分析最需要的证据。

**Plugin 管理**
- **FR-005**: 系统必须支持上传 WASM 文件并填写：名称、标识符（唯一）、Manifest、Runtime 要求（Extism 版本）、版本号、作者、仓库地址。
  **上传期静态校验**（hard requirement）：
  - WASM magic bytes 检查（`\0asm` + version）— 拒绝非 WASM 二进制
  - 文件大小 ≤ `PLUGIN_MAX_BYTES`（默认 16 MB）
  - 扫描 `imports` 段：所有 host function 必须在宿主注册表内；出现未注册 import → 拒绝上传（防止 Plugin 试图绑定未来 capability）
  - 忽略 Plugin manifest 中的 `allowed_hosts` / `allowed_paths` 字段（防止 Plugin 通过 manifest 自我提权）
  - 计算并存储 sha256 + 文件大小
- **FR-006**: 系统必须支持对 Plugin 的添加、修改、软删除（标记 `deleted_at`）。
- **FR-007**: 系统必须在 Plugin 被任何未删除 Function 引用时拒绝软删除。
- **FR-008**: WASM 文件必须存储到 S3 兼容对象存储；DB 仅持有引用键 + 校验和。
- **FR-009**: 系统必须支持"分类 + 标签 + 关键词" 三维组合检索，关键词需覆盖名称/标识符/描述。

**Function 管理**
- **FR-010**: Function 分为"内置（builtin）"与"定制（custom）"两类；内置由系统代码注册，不可删除，可被禁用。MVP 内置集合：`format.template`、`json.parse`、`json.stringify`、`text.regex_match`、`chat.respond`（前 4 个是 Workflow 胶水，最后一个用于生成用户可见的最终回复）。
- **FR-011**: 定制 Function 必须强引用一个未删除的 Plugin 与其某个 export 名。
- **FR-012**: Function 字段：id、type、name、identifier（唯一）、description、input_schema（JSON Schema）、output_schema（JSON Schema）、关联 plugin_id、plugin_export、created_at、updated_at。
- **FR-013**: 系统必须在保存 Function 时校验 input/output schema 为合法 JSON Schema（draft 2020-12 或同等）。

**Workflow 管理**
- **FR-014**: Workflow 必须以 DAG 表示，节点是 Function 引用，边携带"上游某 output 字段 → 下游某 input 字段"映射。
- **FR-015**: Web 必须提供可视化编辑器以拖拽节点与连线。
- **FR-016**: 系统必须在保存时拒绝包含环的 DAG。
- **FR-017**: Workflow 执行必须按拓扑顺序，独立分支并行执行；任一节点失败终止流程并返回错误（含节点 id）。

**Tool 管理**
- **FR-018**: Tool 字段：id、type（function / workflow）、name、identifier、description、input_schema、output_schema、引用的 function_id 或 workflow_id、created_at、updated_at。Tool 在 runtime 通过 `crates/agent::ToolRegistry` 暴露给 LLM；workflow-wrapped Tool 在 ToolRegistry 上同样表现为单一 `Tool` impl，内部 dispatch 到 Workflow 执行器。
- **FR-019**: 单 Function Tool 的 schema 必须等于其引用 Function 的 schema。
- **FR-020**: Workflow-wrapped Tool 的 input_schema = DAG 入口 Function 的 input_schema；output_schema 由编辑者显式声明（默认为 DAG 终点的 output_schema）。

**Skill 管理**
- **FR-021**: Skill 内容以 markdown 形式持久化（含可选 YAML frontmatter）。调用 Skill = 把其 markdown 内容拼接到当前 Agent 的 system prompt 末尾；Skill **不**作为 OpenAI tool 暴露给 LLM。复用 `crates/agent::SkillsLoader` 的现有模式。Skill 可在文本中以模板插值方式引用 Function / Workflow 调用结果，但 Function/Workflow 的执行最终走 ToolRegistry。

**Agent 管理**
- **FR-022**: 入口 Agent 标识固定为 `main`，必须存在且不可删除（删除 API 必须拒绝）。对 `main` Agent 的任何字段修改（含 system_prompt / tools / skills / permissions）仅 Super 角色可执行；非 main Agent 的 CRUD = System 及以上；任何 Agent 的 permissions 含 `is_dangerous=1` 的 capability 时，保存动作仍要求 Super。
  **危险 capability 的赋予 + 撤销 + 调用** 都受 Super 关口：
  - **赋予**（PUT /api/agents 把 `is_dangerous=1` 的 capability 加入 permissions）→ 仅 Super
  - **撤销**（PUT /api/agents 把 `is_dangerous=1` 的 capability 从 permissions 移除）→ 仍仅 Super（避免 System 在 Super 不在场时"先撤后改"绕过审计）
  - **调用**（运行时 host_call 走该 capability）→ dispatcher 按当前 Agent.permissions 鉴权（与赋予者无关）
  CapabilityPicker UI 规则（与 003 "无权限 = 不渲染" 约定一致）：
  - **System 角色**打开 CapabilityPicker → `is_dangerous=1` 的项**不渲染**（不是 disabled）。System 看不到、点不到、也无法通过 DevTools 强行勾选并提交（后端再次校验拒绝）。
  - **Super 角色**打开 CapabilityPicker → 所有 capability 都渲染；dangerous 项加 ⚠ 标记 + tooltip 解释风险。
  - 后端是权威：即便前端被绕过强行 POST 含 dangerous capability 的请求，service 层在保存时按调用者角色再校验一次（非 Super → 2001 InsufficientPermission）。
- **FR-023**: Agent 可嵌套至多 10 层；新建子 Agent 时校验深度。
- **FR-024**: Agent 字段：id、name、description、created_at、updated_at、tools（多对多 Tool）、skills（多对多 Skill）、system_prompt、permissions（capability 名集合）、parent_agent_id（NULL 表示根）、`model_preset`（可选；指向 hiveweb 启动时从 `llm_presets.toml` 加载的命名 preset；为空时 fallback 到全局默认 preset；子 Agent 不继承父的 preset）。
- **FR-025**: Agent 路由由 LLM tool-calling 自决（系统组装 system_prompt + 子 Agent 列表 + tools/skills 后由 LLM 选择 `route_to_subagent(agent_id, reason)` / 直接回复 / 调用 Tool）。宿主在此之上叠加 hard-rule 安全门：① 深度 ≥ 10 拒绝路由；② 同会话路径出现循环立刻终止（最大跳次见 §Edge Cases）；③ 越权 capability 直接 4030 拒绝；④ `route_to_subagent(agent_id, ...)` 的 `agent_id` 必须是当前 Agent 的**直接子 Agent**（不能跨级跳，防止越级访问）。
  **子 Agent 错误的脱敏上报**：子 Agent 内部抛错时，宿主仅向父 Agent / SSE client 暴露 `{ code, message }` envelope（message 为用户文案，见 contracts/api.md §Errors）；**不**暴露子 Agent 的 system_prompt、内部 tool 名、Plugin identifier、stack trace、内部 capability 名（防止信息泄漏给恶意构造提示词的用户）。完整内部错误信息只写 audit log（含 request_id 供运维追溯）。
- **FR-026**: Agent 调用 LLM、Tool、Skill 时均必须先通过 Capability 鉴权；越权立即返回错误。

**测试聊天**
- **FR-027**: 管理员可通过测试窗口与 `main` Agent 对话；后端持久化会话历史，至少在同一登录会话内可见。
  **会话所有权与隔离**（hard requirement）：每个 `chat_sessions` 行绑定 `admin_id`（创建时从 JWT claims 取）；所有 `/api/chat/sessions/:id/*` 端点（GET messages、POST messages、DELETE session 等）必须校验：当前 JWT 的 admin_id == session.admin_id，**否则 403**（即使 token 有效）。Super 角色可读其它管理员的 session（审计用途）但仍走显式 403/200 判定，不允许任何路径绕过所有权检查。
  **Rate-limit 兼容**（CHK232）：复用 003 的 `tower-http RateLimit` 中间件对 SSE 长连接不友好（每个 token 事件计为一次请求会被误杀）。`POST /api/chat/sessions/:id/messages` 端点在 router 装配时**绕过** RateLimit 中间件，改用独立的 per-admin SSE 并发上限：默认每个 admin 同时 ≤ 2 个 active SSE 流（`CHAT_SSE_MAX_CONCURRENT_PER_ADMIN` env），超出 → 立即返 429 + `code = 4291` 而非排队。
- **FR-028**: 聊天端点 `POST /api/chat/sessions/:id/messages` 必须以 SSE（text/event-stream）返回。
  **事件类型与顺序**：
  - `token`（0..N 次）— 增量文本片段；按时间顺序，可被 `tool_call` 或 `routed` 中断
  - `tool_call`（0..N 次）— Agent 决定调用 Tool；服务端**在此事件后暂停 `token` 流**直到 tool 执行完
  - `tool_result`（与 `tool_call` 配对）— Tool 执行完成的结果摘要
  - `routed`（0..N 次）— 切换到子 Agent；后续 `token` 来自新 Agent；服务端写入 `routed_to_agent_id` 字段到 chat_messages
  - `fallback_used`（0..N 次）— LLM provider 失败已切到 fallback 节点；`{ from, to, reason }`；client 可忽略
  - `done`（必有且仅 1 次，整流终点）— 含 final assistant 消息 / elapsed_ms / final_agent_id；客户端即使不消费中间事件也能仅从 `done` 重建完整内容
  - `error`（与 `done` 互斥）— 整链失败时发出，含 code（4030/4040/5003/5004/...）+ message；任一发出后流即结束
  **客户端 UX 契约**：
  - 提交后到第一个 `token`/`tool_call` 之间：UI 显示"正在思考…"占位符 + spinner；30 秒无任何事件视为超时并降级展示错误
  - SSE 连接中途断开：UI 在已渲染内容下方显示"连接中断"提示 + "重新发送"按钮；user 消息已 persist（仍可在历史中看到），assistant 中断的内容**不**写库（避免半截消息污染历史）
  - `tool_call` 期间：UI 显示该 tool 名称 + 入参摘要的卡片，等 `tool_result` 到达后折叠
  - `routed` 触发：UI 显示一行"已切换到 {agent_identifier}：{reason}"分隔条，后续 token 视觉上归属新 Agent

**Runtime 实现**
- **FR-029**: WASM 执行使用 Instance Pool：编译好的 Plugin 实例缓存复用，避免重复编译；池大小可配置，超时空闲实例回收。**实例可跨 Agent / 跨 Session 复用**，但每次归还池前必须 reset linear memory；Plugin 作者不得依赖 mut global 在不同 host_call 之间持久化状态（违反此约束的行为视为 Plugin bug，不在宿主兜底范围内）。
  **加载前 sha256 校验**（hard requirement）：宿主从对象存储 GET WASM 文件后、编译实例前，必须重新计算 sha256 并与 DB 中存储值比对；不一致 → 拒绝实例化 + 写 audit + 通知运维（防止对象存储被外部篡改）。
  **Pool 容量与行为**（CHK211 / CHK213 / CHK214）：
  - 默认上限：每 Plugin 最多 8 实例（`PLUGIN_POOL_MAX_PER_PLUGIN`），全局总数 64（`PLUGIN_POOL_MAX_TOTAL`）
  - 空闲回收：实例闲置超过 10 分钟（`PLUGIN_POOL_IDLE_TIMEOUT_SEC`）由后台任务释放
  - **Pool 满时 acquire 行为**：先按 FIFO 等待空闲实例最多 5 秒（`PLUGIN_POOL_ACQUIRE_TIMEOUT_MS`）；超时 → 返回 `5009 PoolBusy` 错误而非阻塞 chat session
  - **实例 reset 失败处理**：归还前 reset 抛错 → 丢弃实例（不入池）+ 计入 `pool.reset_failures` 指标 + audit 一条 `outcome=error`；下次 acquire 会重新编译一个新实例（cache_miss 计数 +1）
- **FR-030**: 单次 Plugin 调用必须有硬超时，默认 **30 秒**（可配 `PLUGIN_CALL_TIMEOUT_MS` env），超时强制中止并归还实例位。
  **双层 timeout 实现**（CHK215）：① Wasmtime **fuel-based budget** 防止纯 CPU 死循环（按指令计数中断；上限由 `PLUGIN_CALL_FUEL` env 调，默认值与 30s × 典型指令吞吐对齐）；② tokio `select!` 包一层墙钟超时防止 await 永远不返回（如 host_call 内部网络挂起未响应）。两者任一触发即视为 timeout，宿主返 5004 + audit + 丢弃实例（被中断的 Wasmtime 实例不再可信，不入池）。
- **FR-031**: 单次 Plugin 调用必须有内存上限，默认 **128 MB**（可配 `PLUGIN_CALL_MAX_MEMORY_MB` env），超限中止。

### Key Entities

- **Capability**：固定枚举集合，由宿主代码定义，不入库。其元数据（描述、is_dangerous）可入库以便 UI 展示。
- **Category**：树状分类，自引用 parent_id，支持 Plugin / Function 分组。
- **Tag**：扁平标签集合。
- **Plugin**：WASM 二进制 + 元数据。多对一 Category；多对多 Tag。
- **Function**：可执行单元。多对一 Plugin（仅定制）；多对一 Category；多对多 Tag。
- **Workflow**：DAG。多对多 Function（通过 WorkflowNode）。
- **WorkflowNode + WorkflowEdge**：DAG 的节点 / 边。
- **Tool**：包装层。引用 Function 或 Workflow。
- **Skill**：上层封装。引用 Function 或 Workflow + 自有描述/示例。
- **Agent**：交互单元。自引用 parent_agent_id；多对多 Tools；多对多 Skills；Set 列存 permissions。
- **ChatSession + ChatMessage**：测试聊天的会话与消息。
- **AuditLog（runtime）**：每次 capability 调用、每次 Agent 路由、每次 Workflow 节点执行的审计记录。

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 上传一个 1MB 内 Plugin → 入库 + 落对象存储完成耗时 ≤ 5 秒（p95）。
- **SC-002**: Plugin 列表（≤ 500 条）+ 三维过滤 + 全文检索 p95 ≤ 1 秒。
- **SC-003**: Workflow 编辑器在 ≤ 50 节点的图上保存/校验 p95 ≤ 1 秒。
- **SC-004**: 单次 host_call 鉴权 + 转发开销 p95 ≤ 5 毫秒（不含目标资源耗时）。**度量边界**（CHK225）：从 dispatcher 收到 envelope 字节起，到准备调用 capability handler 之前（含 JSON 反序列化 / Agent.permissions 查询 / capability 名查表）；不含 handler 内部的网络 / DB / S3 调用耗时。压测工具：tokio `Bencher` 直接调 dispatcher，绕过 HTTP 层。
- **SC-005**: 在 Instance Pool 命中的情况下，一次 Plugin 函数调用（不含目标资源耗时）p95 ≤ 50 毫秒；冷启动（无命中）p95 ≤ 300 毫秒。**度量定义**（CHK226）：**命中** = `PluginPool::acquire(plugin_id)` 返回 `Ok(guard)` 且 guard 来自 `idle` 队列（命中计数器递增）；**冷启动** = pool 中无该 plugin_id 任何实例 → 触发 Wasmtime 编译 + 首次 `Plugin::new`。`reset` 后归还的实例**仍算命中**（reset 是常量时间）。压测脚本：先 warmup（连续调 8 次同一 Plugin 让池满），然后取后续 100 次调用的 p95。
- **SC-006**: Agent 路由决策 p95 ≤ 1.5 秒（含一次 LLM 决策调用）。
- **SC-007**: 越权 Capability 调用 100% 被宿主拒绝并审计，无任何漏放。
- **SC-008**: `main` Agent 删除尝试 100% 被拒绝。
- **SC-009**: Plugin 软删除前的引用检查 100% 准确（被引用时 0% 误删）。**Race window 规避**：软删除 transaction 必须 `SELECT ... FOR UPDATE` 锁住目标 Plugin 行 + `SELECT COUNT(*) FROM functions WHERE plugin_id = ? AND ...` 同事务内查；锁释放前并发的"新建 Function 引用此 Plugin" 必须 `SELECT plugin FOR SHARE`（应用层）— 否则视为竞态读到旧"未删除"状态，service 层 INSERT 时再查一次 `deleted_at` 并 rollback。两层防御。
- **SC-010**: 一次完整对话（入口 → 路由 → 子 Agent → 一次 Tool 调用 → 回复）端到端 p95 ≤ 8 秒（含 1 次 LLM 调用）。

## Threat Model

004 引入 WASM Plugin runtime + 多模型 LLM 调用 + 多级 Agent，扩展了 003 的攻击面。本节列出本特性显式纳入考虑的威胁与对应缓解 ——**未列出的威胁视为 out of scope 或继承自 003**。

### TM-1 恶意 / 错误 Plugin（assume hostile plugin author）

**威胁**：Plugin 作者上传含恶意逻辑的 WASM —— 试图访问宿主资源、发起 SSRF、读取其它 Plugin 的状态、绕过 capability。
**缓解**：
- WASM 默认无 WASI 系统调用 → 只能通过显式注册的 host function（FR-002）
- Capability dispatcher 在 host_call 入口零信任鉴权 + 拒绝路径强制审计（FR-003 / FR-004）
- Instance Pool 归还前强制 reset linear memory（FR-029）
- `network.http` 强制 SSRF 防护（FR-001 v6）
- Plugin 上传期 import 静态检查（FR-005 v6）
- WASM 加载前 sha256 重新校验（FR-029 v6）

### TM-2 越权管理员（assume System role 操作恶意 / 失误）

**威胁**：System 角色试图给 Agent 授予危险 capability、删除 main、修改 main 的 system_prompt、跨用户访问 chat session。
**缓解**：
- 危险 capability 的赋予 / 撤销 = Super only（FR-022 v6）；System 在 UI 上看不到（hidden 而非 disabled）+ 后端再校验
- `main` Agent 删除拒绝（FR-022）+ 修改限 Super
- chat session 绑定 admin_id（FR-027 v6）
- 所有变更走 audit log（FR-004）

### TM-3 凭据 / 密钥泄漏

**威胁**：JWT_SECRET / LLM API KEY / DB 密码 / `secret.get` capability 泄漏。
**缓解**：
- 沿用 003：env-injected，CI scan，DB URL 日志遮码
- `secret.get` allowlist（contracts/host-functions.md §4.6）+ 调用审计
- 错误响应不携带 stack trace（FR-025 v6 子 Agent 错误脱敏；通用 capability handler 同此原则）
- audit log 中 payload_summary 按字段黑名单脱敏（data-model §V017）

### TM-4 DoS / 资源耗尽

**威胁**：Plugin 死循环 / OOM；高并发刷 host_call；超大 WASM 文件；超长 chat content。
**缓解**：
- Plugin 单次 timeout 30 s（FR-030）+ memory 128 MB（FR-031）+ fuel-based 中断
- Pool 大小 + 等待超时（research §2）
- Plugin 文件 ≤ 16 MB（spec §Edge Cases）
- chat content ≤ 64 KB（contracts/api.md §8）
- rate limit 沿用 003

### TM-5 数据外泄（含 LLM 提示词注入）

**威胁**：用户构造提示词让 LLM 调用越权 tool；子 Agent 错误回流给用户暴露内部 prompt；audit 日志被读出敏感字段。
**缓解**：
- Capability 鉴权对 LLM 决策不可逆（不论 LLM 输出什么，host_call 进入 dispatcher 仍按 Agent permissions 判定）
- 子 Agent 错误脱敏上报（FR-025 v6）
- audit log 不可篡改（append-only）+ 90 天后归档；非 Super 不可读其它管理员的会话/审计（FR-027 v6 + 003 权限模型）

### Out of Scope（继承自 003 或工程基础保障，不在 004 spec 重复约定）

- 中间人 / 网络层加密（依赖 TLS / VPC）
- 物理 / 主机层入侵（依赖基础设施安全）
- Wasmtime / Extism / providers crate 本身的 0-day（依赖上游修复 + 依赖扫描 CI）
- 管理员 PC 被入侵导致 JWT 泄露（依赖端点安全）

## Assumptions

- LLM 调用一律走 workspace 中既有的 `crates/providers`（多 backend 抽象 + Fallback + tool-calling）。Agent Runtime 不直接持 vendor SDK，只通过 `LLMProvider` trait 调用。
- LLM 失败回退由 `providers::FallbackProvider` 链承担；启动配置以 preset 形式给出 primary + fallback 列表（如 `primary=openai-compat, fallback=[bedrock, anthropic]`）。仅当整条链全部失败时，错误才作为 SSE `error` 事件传给客户端。
- 同时支持多个命名 preset（如 `cheap-fast` / `code-expert` / `claude-fallback`），每个 Agent 通过 `model_preset` 字段选择；为空 fallback 到全局默认 preset。
- 已有的管理中心（003-admin-center）提供的角色与认证体系直接复用；Runtime 端点保护沿用同一 JWT。
- 对象存储沿用既有 Rustfs（S3 兼容）部署，不引入新存储。
- Plugin 作者用 Extism 官方 SDK（Rust/Go/JS/Python 等任一）；宿主仅承诺 Extism PDK 兼容。
- DAG 编辑器使用前端通用图编辑库（具体选型在 plan）。
- 内置 Function 由系统启动时静态注册；不支持热加载内置函数。
- 聊天会话历史保留期默认 **30 天**（可配 `CHAT_RETENTION_DAYS` env）；cron 任务每日清理超期 session（含级联 message）。
- Capability `db.execute` / `db.query` 仅允许宿主预注册的命名查询/写入（zero-trust），自由 SQL 永不暴露 — 已锁定。

## Outstanding Clarifications

✅ 全部 12 项已通过 4 轮 `/speckit-clarify` 收敛，决议记录于 §Clarifications。本节保留为历史索引：

| # | 主题 | 决议 |
|---|------|------|
| 1 | LLM 提供商 | 复用 `crates/providers` 多 backend 抽象（含 Anthropic / OpenAI 兼容 / Bedrock / Codex / Copilot / Azure / Fallback） |
| 2 | Agent 路由决策 | LLM tool-calling 自决 + hard-rule（深度 ≤ 10 / 循环 / 越权拒绝） |
| 3 | Plugin 多版本 | 共存；唯一 `(identifier, version)`；Function 绑定具体版本主键 |
| 4 | 聊天历史保留 | 30 天默认，可配 `CHAT_RETENTION_DAYS` env |
| 5 | 路由跳次上限 | 5（默认，可配 `AGENT_MAX_HOPS` env） |
| 6 | Plugin 文件大小 | 16 MB（默认，可配 `PLUGIN_MAX_BYTES` env） |
| 7 | `db.execute` 范围 | 仅宿主预注册的命名查询/写入；自由 SQL 永不暴露 |
| 8 | 流式回复 | 强制 SSE |
| 9 | Plugin 调用 timeout / memory | 30 秒 / 128 MB（默认，可配 env） |
| 10 | 并发编辑冲突 | 乐观锁 `updated_at`；冲突返回 409 |
| 11 | 子 Agent capability 边界 | 不继承；按当前执行中的 Agent 鉴权 |
| 12 | Instance Pool 跨会话复用 | 是，归还前强制 reset |
| 13 | Tool vs Skill | Tool 走 OpenAI tool-calling；Skill 拼 system_prompt |
| 14 | Per-Agent LLM 选型 | `model_preset` 字段，NULL → 全局默认 |
| 15 | LLM 失败回退 | 强制 FallbackProvider 链 |
| 16 | Agent CRUD 权限 | main 编辑限 Super；危险 capability 赋予限 Super |
| 17 | 内置 function 集合 | `format.template` / `json.parse` / `json.stringify` / `text.regex_match` / `chat.respond` |
