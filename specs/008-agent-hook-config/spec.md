# Feature Specification: Agent Hook 配置管理

**Feature Branch**: `008-agent-hook-config`
**Created**: 2026-06-02
**Status**: Draft
**Input**: User description: "在管理中心为Agent配置hook"

## Clarifications

### Session 2026-06-02

- Q: 每个 Agent 最多允许配置多少个 Hook？ → A: 每个触发点最多 5 个 Hook，总 Hook 数不限（由各触发点独立限制）
- Q: Webhook 调用失败后是否需要自动重试？ → A: 仅对 eligible transient failure 后台异步重试最多 3 次（固定 1/2/4 秒退避），不阻塞 Hook 串行执行流；最终结果输出结构化 tracing。可重试集合由 FR-007a 封闭定义。
- Q: 非阻塞模式 Hook 失败时，后续 Hook 应继续还是跳过？ → A: 始终继续执行后续 Hook（非阻塞模式下各 Hook 相互独立，单点失败不影响后续）
- Q: Hook 执行结果是否应注入 Agent 的 system_prompt 或修改运行时状态？ → A: 否。Hook 为只读观察者，不修改 Agent 状态。（该“严格只读”结论由 2026-07-24 的受控 `AgentContext` 更新决议取代；保留本条作为历史记录。当时采用的持久化审计方案亦已由 2026-07-16 决议取代。）

### Session 2026-07-16

- Q: Hook 执行记录是否继续持久化到数据库？ → A: 否。为避免执行量增长导致数据库容量压力，Hook 执行结果仅输出结构化 tracing；不再持久化执行历史，也不再提供执行历史查询界面或接口。已有历史表和历史数据保持不动。

### Session 2026-07-23

- Q: 对外 Agent 执行错误是否继续沿用已移除的管理聊天 SSE 契约？ → A: 否。HiveWeb 对外执行入口是同步 JSON `POST /api/assistant`；阻塞 Hook 超时返回 6004/HTTP 408，其他阻塞失败返回 6005/HTTP 500。失败请求只回滚一次配额，不执行 `after_agent_end`，不持久化 assistant 占位消息，但仍以 tracing-only 方式触发 `on_agent_error`。
- Q: 哪些终止错误触发 `on_agent_error`，以及该阶段是否需要总预算？ → A: 每个 orchestration hop 成功加载 Agent 内容后，会为该 hop 固定 Hook 快照；该快照之后发生的所有终止 Agent 错误（包括模型预设解析、LLM、阻塞 Hook、路由循环、最大跳数，以及下一 hop 的 Agent 内容加载失败）都触发 tracing-only `on_agent_error`。下一 hop 重新获取 Agent 内容并在成功后替换快照；后续加载失败使用最近一次成功快照，首次加载失败、尚无快照时明确跳过。整个 `on_agent_error` 阶段共享固定 30 秒总预算；失败或预算耗尽仅记录脱敏 tracing，原始错误保持不变且不得递归触发。

### Session 2026-07-24

- Q: 普通 Assistant 执行与 Hook 嵌套执行如何共享关联 ID，同时保证 Hook 子链不会重新启用数据库审计？ → A: `POST /api/assistant` 在边界构造唯一的显式 `RuntimeExecutionContext`，携带本次 `request_id` 与服务端 `session_id`；普通 Agent 调用树使用 tracing + 有界 best-effort DB audit。进入 Hook 时必须通过 `for_hook()` 派生上下文：关联 ID 原样保留，审计模式单向降级为 tracing-only。该降级对 Hook → Workflow → Plugin → Capability 整条嵌套调用链保持 sticky，任何子层都不得通过默认值、反序列化、payload 参数或重新构造普通上下文来恢复 DB audit。
- Q: Hook 是否仍必须是严格只读观察者？ → A: 否。Function/Workflow 动作会收到当前 `AgentContext` 的只读 `_agent_context` 快照；动作只能通过返回固定形状的 `_agent_context_updates`，由运行时调用受控 `AgentContext` API 更新当前执行中的 records、extensions 或字符串 metadata。其他返回字段不得修改上下文，也不得直接覆盖持久化 Agent 配置或 system prompt。该通道只更新本次运行的内存上下文，不提供数据库写入旁路；动作若需业务副作用，仍必须遵守被选 Function/Workflow 的既有合同，经过 Plugin/Capability 的调用继续接受 Agent permission 与 Capability 鉴权。Hook 子链的审计继续保持 sticky tracing-only。
- Q: 客户端提供的 `X-Request-Id` 是否可原样进入 Hook tracing？ → A: 否。HTTP middleware 只接受固定 36 字节 canonical UUID（大小写输入统一输出为小写，nil UUID 拒绝）；缺失或不合格值在边界替换为新的 UUID。后续响应回显、`RuntimeExecutionContext`、HookContext 和所有 tracing 只使用规范化值，原始不可信 header 不得进入日志。
- Q: Webhook timeout 与后台重试如何避免相同 deadline 的取消竞态？ → A: Webhook 不再由 `run_hooks` 叠加同期限时器；每次 attempt 使用唯一 Tokio timeout 覆盖 DNS 解析、全部地址校验、client 构造和 send。首次 timeout 在返回 Hook timeout 前先作出后台重试调度决定；每次 retry 获得独立的完整 attempt 预算。`HOOK_WEBHOOK_RETRY_MAX` 只接受 0..=3，0 禁用重试，无效值回退规范默认 3；退避固定为 1/2/4 秒。

## User Scenarios & Testing *(mandatory)*

### User Story 1 - 为 Agent 绑定生命周期钩子 (Priority: P1)

管理员在 Agent 编辑页面中，为指定 Agent 配置钩子（Hook），钩子在 Agent 执行过程中的关键生命周期节点自动触发，执行预定义的动作（如调用函数、发起 HTTP 回调等）。

**Why this priority**: 这是 Hook 功能的核心价值——让管理员能够在不修改代码的情况下，为 Agent 的执行流程注入自定义行为（如审计增强、外部通知、预处理/后处理逻辑），是整个功能的基础。

**Independent Test**: 创建一个 Agent → 为其配置一个 `before_agent_start` 钩子（动作=调用内置函数 `format_template`）→ 触发该 Agent 执行对话 → 验证钩子动作被执行并产生结构化可观测性事件，且数据库没有新增 Hook 执行记录。

**Acceptance Scenarios**:

1. **Given** 管理员具有 System+ 角色且 Agent 非 main, **When** 在 Agent 编辑页面的 Hook 配置区域点击"添加 Hook"并选择一个触发点（如 `before_agent_start`）和一个动作类型（如 `call_function`），填写必要参数后保存, **Then** Hook 配置成功保存，Agent 详情中展示该 Hook 的触发点和动作摘要。
2. **Given** Agent 已配置一个 `after_agent_end` 钩子, **When** 该 Agent 执行完成一次对话, **Then** 钩子动作被自动执行，执行结果（成功/失败）输出为结构化可观测性事件且不持久化执行记录。
3. **Given** Agent 已配置多个 Hook（如 `before_agent_start` + `after_agent_end`）, **When** Agent 执行对话, **Then** 所有 Hook 按生命周期顺序依次触发，互不干扰。
4. **Given** Agent 已配置 Hook, **When** 管理员删除该 Hook, **Then** 后续 Agent 执行不再触发该 Hook。
5. **Given** 阻塞模式 Hook 执行失败或超时, **When** 用户调用 `POST /api/assistant`, **Then** 返回统一 JSON 错误 6005/HTTP 500 或 6004/HTTP 408，配额恰好回滚一次，不执行 `after_agent_end`，且不保存 assistant 占位消息。

---

### User Story 2 - 通过 HTTP Webhook 与外部系统集成 (Priority: P2)

管理员配置 Hook 动作为 HTTP 回调（Webhook），在 Agent 执行的关键节点向外部系统发送事件通知。

**Why this priority**: HTTP Webhook 是 Agent 与企业现有系统（如监控告警、工单系统、数据管道）集成的关键通道，扩展了 Agent 的生态连接能力。

**Independent Test**: 为 Agent 配置 `after_agent_end` Hook（动作=HTTP Webhook，URL 指向测试端点）→ 触发 Agent 对话 → 验证外部端点收到包含 Agent 执行摘要的 HTTP POST 请求。

**Acceptance Scenarios**:

1. **Given** 管理员配置了一个 HTTP Webhook 类型的 Hook, **When** 填写回调 URL、可选的请求头和超时时间, **Then** Hook 配置保存成功。
2. **Given** HTTP Webhook Hook 已配置, **When** Hook 触发时, **Then** 系统向目标 URL 发送 POST 请求，请求体包含触发上下文（Agent 标识、会话 ID、触发事件类型、时间戳）。
3. **Given** HTTP Webhook 的目标 URL 不可达或超时, **When** Hook 触发, **Then** 系统输出脱敏的结构化失败事件，不阻塞 Agent 主流程继续执行。
4. **Given** 管理员配置 Webhook, **When** 创建或更新动作类型/参数, **Then** 系统解析 URL、要求 HTTPS、拒绝凭据/云元数据名称/任一非公网 DNS 答案；执行与每次重试重新解析并固定已校验地址，且不跟随重定向。

---

### User Story 3 - 通过 Hook 调用 Workflow/Function 实现自动化 (Priority: P2)

管理员配置 Hook 动作为调用已有的 Workflow 或 Function，实现 Agent 执行前后的自动化处理（如补充当前对话的结构化上下文、生成扩展卡片或触发受权限约束的数据处理流水线）。动作读取 `_agent_context` 快照；如需影响后续运行步骤，只能返回受控 `_agent_context_updates`，由运行时更新当前执行的 `AgentContext`，不能直接覆盖持久化 Agent 配置或 system prompt。

**Why this priority**: 复用平台已有的 Function/Workflow 资产，使 Hook 能执行复杂业务逻辑，最大化现有投资价值。

**Independent Test**: 配置 `before_agent_start` Hook（动作=调用 Function `format_template`）→ 触发 Agent 对话 → 验证 Hook 执行成功、产生结构化成功事件，并且数据库没有新增 Hook 执行记录。

**Acceptance Scenarios**:

1. **Given** 管理员选择 `call_function` 动作类型的 Hook, **When** 从 Function 列表中选择目标 Function 并填写入参映射, **Then** Hook 配置保存成功，参数通过 JSON Schema 校验。
2. **Given** 管理员选择 `call_workflow` 动作类型的 Hook, **When** 选择目标 Workflow 并配置入参来源（可从触发上下文中提取）, **Then** Hook 配置保存成功。
3. **Given** Hook 调用的 Function 执行失败, **When** Hook 触发, **Then** 错误被记录为脱敏的结构化事件，Hook 失败不阻止 Agent 主流程（除非 Hook 配置为"阻塞模式"）。
4. **Given** Hook 调用的 Function/Workflow 返回合法 `_agent_context_updates`, **When** 运行时处理动作结果, **Then** allowlisted records、extensions 与字符串 metadata 被应用到当前 `AgentContext`，后续运行步骤可读取；其他输出字段不产生上下文修改，且不会因此新增数据库审计记录。

---

### Edge Cases

- **Hook 执行超时**：单个 Hook 执行超时（默认 10 秒，可配置）后强制中止该次 Hook。非阻塞模式记录结构化 timeout tracing 后继续 Agent 主流程；阻塞模式中止本次 Agent 执行，并由 `POST /api/assistant` 返回 6004/HTTP 408。HTTP Webhook 的单一 Tokio timeout 覆盖 DNS → client → send 全流程，`run_hooks` 不再对 Webhook 套第二个同期限时器；首次 timeout 在返回前确定重试策略。
- **Webhook 重试**：HTTP Webhook 仅在 `ResolveUnavailable`、reqwest connection error（`is_connect()`）或 attempt timeout 三类 transient failure 后进入后台重试（默认最多 3 次，固定间隔 1s/2s/4s）。`HOOK_WEBHOOK_RETRY_MAX` 严格限制在 0..=3：0 禁用，无效/越界值安全回退默认 3。每次 retry 都有独立完整 timeout 预算；重试期间后续 Hook 照常执行。每次重试及最终结果均输出结构化事件，但不写入数据库。每触发点最多 5 个 Hook 与既有串行语义不变，不新增跨 Hook 并发合同。
- **Webhook DNS/redirect 变化**：创建时安全的域名若在执行或重试时解析出任一私网、loopback、link-local、非公开 IPv6 或 metadata 地址，则该次请求在连接前拒绝。连接必须固定到本次完整校验的 DNS 答案，响应 3xx 不跟随，不能通过 DNS rebinding 或 redirect-to-private 绕过。
- **Hook 配置更新时机**：Hook 创建、更新或删除成功后，系统立即以 best-effort 方式失效对应 Agent 的内容缓存。当前 hop 已固定的 Hook 快照不变；下一 hop（包括同一会话内的下一 hop）重新调用 `fetch_content`，并在成功后使用新快照。缓存失效失败只输出不含 Redis 原始错误文本的静态分类 tracing，不覆盖已经成功的 Hook 变更；后续 Redis miss、不可用时的 DB fallback 或 TTL 到期仍会收敛到新配置。
- **多个 Hook 顺序**：同一触发点可配置多个 Hook，按其配置序号依次串行执行。非阻塞模式下各 Hook 相互独立，单点失败不阻止后续 Hook。阻塞模式下失败立即中止。
- **Hook 动作引用失效**：Hook 引用的 Function/Workflow 被删除时，Hook 配置状态变为"失效"，触发时跳过并输出结构化警告事件。
- **循环引用检测**：Hook 触发的 Function/Workflow 如果携带可能导致递归触发 Agent 的操作，系统在配置保存时不检测（运行时由 Agent 最大跳次防护兜底）。
- **main Agent Hook 变更权限**：与现有 Admin Center 权限模型一致——非 Super 角色无法修改 main Agent 的任何配置（含 Hook），UI 中 Hook 配置区域不渲染。
- **并发执行**：同一 Agent 的多个 Hook 按配置顺序串行执行，避免并发带来的状态管理复杂性。
- **Hook 数量上限**：每个触发点最多配置 5 个 Hook。保存时若超出上限，系统拒绝并提示"该触发点最多配置 5 个 Hook"。
- **on_agent_error 终止错误覆盖**：当前 hop 的 Hook 快照成功加载后，模型预设解析失败、LLM 错误、阻塞 Hook 错误、路由循环、最大跳数和后续 hop 的 Agent 内容加载失败等终止错误均触发 `on_agent_error`。整个阶段共享固定 30 秒总预算，而不是每个 Hook 各自获得 30 秒；阶段失败或预算耗尽不得覆盖原始错误，也不得递归触发 `on_agent_error`。
- **on_agent_error 提前失败路径**：若首次 Agent 内容加载阶段即失败（如 DB 连接错误），此时 Hook 快照尚未加载，`on_agent_error` 明确跳过。此场景仅输出原始错误事件，不追加 Hook 执行事件；若之前已有成功加载的 hop 快照，则后续 Agent 内容加载失败使用最近一次成功快照触发。
- **Hook 审计模式不可升级**：进入 Hook 后，即使动作继续调用 Workflow、Plugin 和 Capability，也只能产生带原始 request/session 关联 ID 的 tracing-only audit；嵌套层不得重新启用 `runtime_audit_logs` 写入。该限制只约束运行时审计副本，不禁止 Hook 配置、Function/Workflow 元数据或业务动作所需的正常数据库访问。
- **受控上下文更新无效项**：缺少 `_agent_context_updates` 时不修改上下文；未知 record category、无法写入的 record/extension 或非字符串 metadata 值会被跳过。运行时只输出固定 `error_kind` tracing，不记录原始更新 payload，也不把单个无效更新升级为任意数据库写入。

## Requirements *(mandatory)*

### Functional Requirements

**Hook 配置管理**

- **FR-001**: 系统 MUST 支持为每个 Agent 配置零个或多个 Hook，每个 Hook 定义触发点（trigger_point）和动作（action）。每个触发点最多 5 个 Hook。
- **FR-002**: Hook 触发点 MUST 覆盖以下 Agent 生命周期节点：
  - `before_agent_start`：Agent 开始执行前触发（在 system_prompt 组装之后、LLM 首次调用之前）
  - `after_agent_end`：Agent 成功完成执行后触发（在最终响应扩展收集和 assistant 消息持久化之前）；阻塞 Hook 失败路径 MUST 跳过
  - `on_agent_error`：当前 hop 的 Hook 快照成功加载后的每个终止 Agent 错误均以 tracing-only 方式触发；下一 hop 加载失败时使用最近一次成功快照，首次加载失败且无快照时跳过；整个阶段共享固定 30 秒总预算，其自身失败或预算耗尽不得覆盖 `POST /api/assistant` 的原始同步 JSON 错误，也不得再次递归触发
  - `before_tool_call`：Agent 决定调用 Tool 时触发（在 capability 鉴权之前）
  - `after_tool_call`：Tool 执行完成后触发
  - `before_llm_call`：LLM 调用发出前触发
  - `after_llm_call`：LLM 响应返回后触发
- **FR-003**: Hook 动作类型 MUST 支持以下三种：
  - `call_function`：调用平台已有的 Function（含内置和定制）
  - `call_workflow`：调用平台已有的 Workflow
  - `http_webhook`：向外部 URL 发起 HTTP POST 请求
- **FR-004**: 每个 Hook 配置 MUST 包含：名称、描述、触发点、动作类型、动作参数、是否启用、排序序号。
- **FR-005**: 系统 MUST 支持 Hook 的启用/禁用开关，禁用的 Hook 不触发执行。
- **FR-006**: 对于 `call_function` 和 `call_workflow` 动作类型，系统 MUST 在保存时校验目标 Function/Workflow 存在且未被删除；引用失效时拒绝保存并提示。
- **FR-007**: 对于 `http_webhook` 动作类型，系统 MUST 校验 URL 合法性：
  - 仅允许 `https://` 协议
  - 拒绝 URL 凭据、云元数据 hostname、IP literal 或 DNS 返回的任一非公网 IPv4/IPv6 地址
  - Hook 创建，以及任何 `action_type` / `action_params` 更新 MUST 对合并后的有效动作执行同一校验，仅更新 `action_params` 不得绕过
  - 执行及每次重试 MUST 重新解析并校验全部 DNS 答案，把请求固定到该批地址，禁用环境代理和 HTTP 重定向；不得发生第二次未校验 DNS lookup
- **FR-007b**: `action_params.headers` 中的自定义 HTTP header 键值对 MUST 使用实际 HTTP header parser 校验，拒绝非字符串值、非法 name/value、CR/LF injection 和 `Host` 覆盖。保存、执行和重试 MUST 使用同一校验规则。
- **FR-007a**: HTTP Webhook 只有以下三类 typed transient failure MUST 进入后台异步重试：① DNS 暂时不可用的 `ResolveUnavailable`；② reqwest connection error（`error.is_connect()`）；③ attempt-local timeout。可重试集合是封闭集合：`Policy`、header 解析、HTTP client build、其他 reqwest `Request` 错误以及任意 HTTP non-2xx 响应 MUST NOT 重试，也不得按错误字符串或 HTTP 状态码范围猜测。`HOOK_WEBHOOK_RETRY_MAX` 仅允许 0..=3（默认 3，0 禁用，无效/越界回退 3），固定退避为 1s → 2s → 4s，不得由指数运算产生溢出。首次 attempt 必须在返回 Hook 结果前同步分类并确定是否调度；重试不阻塞当前 Hook 串行执行流（后续 Hook 照常执行）。每次重试和最终 error 状态 MUST 输出结构化事件，不得持久化执行记录。

**Hook 执行**

- **FR-008**: 当 Hook 触发时，系统 MUST 向 Hook 动作传递触发上下文（Agent 标识、会话 ID、触发事件、时间戳）。
- **FR-009**: Hook 执行 MUST 不阻塞 Agent 主流程——默认采用"异步"模式，Hook 失败仅输出结构化事件，不影响 Agent 继续执行。
- **FR-010**: 系统 MUST 支持 Hook 的"阻塞模式"配置项。当设置为阻塞模式时，Hook 失败 MUST 中止 Agent 当前流程；超时通过 `POST /api/assistant` 返回 6004/HTTP 408，其他失败返回 6005/HTTP 500。错误消息 MUST 为普通文本，不得嵌套序列化 JSON。失败路径 MUST 恰好回滚一次当日配额、跳过 `after_agent_end`，且不得持久化 assistant 占位消息。
- **FR-011**: 单个普通 Hook 执行超时时间 MUST 可配置（默认 10 秒），超时后强制中止并输出结构化超时事件。Function/Workflow 可由 `run_hooks` 施加该预算；HTTP Webhook MUST 改用单个 attempt-local Tokio timeout 覆盖 DNS、client 构造与 send，`run_hooks` MUST NOT 再叠加相同 deadline。每次后台 retry 获得新的完整预算。`on_agent_error` 阶段在单 Hook 超时之外 MUST 具有固定 30 秒总预算；预算耗尽 MUST 取消剩余执行、输出不含下游错误文本的安全 tracing，并保留原始终止错误。
- **FR-012**: 同一触发点的多个 Hook MUST 按排序序号从小到大依次串行执行。非阻塞模式下，单个 Hook 失败不影响后续 Hook 继续执行（各 Hook 相互独立）。阻塞模式下，单个 Hook 失败立即中止 Agent 流程，不执行后续 Hook。
- **FR-013**: Hook 执行时的 capability 鉴权 MUST 沿用当前执行中 Agent 的 permissions 集合。

**审计与可观测性**

- **FR-014**: 每次 Hook 触发执行 MUST 输出结构化 tracing 事件，至少包含：Agent ID、Hook ID、触发事件、动作类型、执行结果（success/error/timeout/skipped）、耗时、请求 ID。
- **FR-015**: 系统 MUST NOT 将 Hook 执行结果、上下文快照或重试结果持久化到数据库，也 MUST NOT 提供 Hook 执行历史查询接口或管理界面。
- **FR-016**: Hook 执行失败时，结构化事件 MUST 只记录白名单错误分类（`error_kind`），不得记录下游返回的任意错误文本、完整用户消息正文、Webhook URL、请求体或响应体。

**显式执行上下文**

- **FR-021**: `POST /api/assistant` MUST 在请求边界构造一个显式 `RuntimeExecutionContext`，携带该请求的规范化 `request_id` 与服务端解析出的 `session_id`。客户端 `X-Request-Id` 仅接受固定 36 字节 canonical non-nil UUID，接受后统一为小写；缺失或不合法时生成新 UUID。middleware MUST 在调用任何下游 middleware/handler 前，以规范化值覆盖请求 `HeaderMap` 中的 `X-Request-Id`；响应回显、handler 读取、Context 与 tracing MUST 只使用同一规范化值，原始不可信 header MUST NOT 进入日志或下游读取路径。普通 Assistant 调用树的 runtime audit MUST 始终先输出结构化 tracing，并可通过有界队列 best-effort 写入数据库；持久化副本失败不得影响 Agent 主流程。
- **FR-022**: Orchestrator 进入 Hook 前 MUST 仅通过父上下文的 `for_hook()` 派生 Hook 执行上下文。派生 MUST 原样保留 `request_id` / `session_id`，并将 runtime audit 单向降级为 tracing-only。该模式 MUST 为 sticky：Hook → Workflow → Plugin → Capability 只能显式传递或克隆同一受限上下文，任何嵌套层不得重新构造普通上下文或恢复 DB audit。
- **FR-023**: `RuntimeExecutionContext` 的审计持久化模式 MUST 是运行时私有状态，不得实现默认构造或从 HTTP、Plugin、Workflow、Hook payload 反序列化。Hook 子链的 Hook 生命周期、Workflow 节点、Plugin 调用和 Capability 调用等结构化审计 tracing MUST 保留父级 request/session 关联 ID，且 MUST NOT 向 runtime audit DB 队列写入记录。

**受控 AgentContext 更新**

- **FR-024**: `call_function` 与 `call_workflow` Hook 动作 MUST 将 `action_params.args` 对象合并到调用输入，并能读取当前执行的可信 HookContext 顶层字段与 `_agent_context` 快照。合并优先级固定为 `args < runtime HookContext < _agent_context`：运行时 HookContext 覆盖 `agent_id`、`identifier`、`session_id`、`actor_id`、`request_id`、`trigger_point`、`message`、`channel`、`client_type`、`client_version` 等同名配置，随后最后注入 `_agent_context`。配置不得伪造任何运行时值。该快照可包含 user input、messages、records 和 extensions，但 MUST 只作为动作输入；系统 MUST NOT 将完整快照写入 Hook 执行历史、runtime audit payload 或任意新持久化实体。
- **FR-025**: Function/Workflow 动作仅可通过输出顶层 `_agent_context_updates` 请求修改当前内存 `AgentContext`。运行时 MUST 只处理以下固定操作：`records` 通过 `set_record` 写入受支持 category，`extensions` 通过 `add_extension` 写入受支持内容类型，`metadata` 通过 `set_metadata` 写入字符串值。缺少该顶层字段或位于其他输出字段中的数据 MUST NOT 修改上下文。该键 MUST 视为内部保留输出键：所有 Workflow 公共 end-output 合成路径以及 HTTP `node_results` 每个节点值的顶层 MUST 剥离它；显式 `output_schema.properties` 声明该键时 MUST 以既有 5005/HTTP 422 和固定安全消息拒绝。
- **FR-026**: `_agent_context_updates` MUST NOT 直接修改持久化 Agent 配置、system prompt 或 runtime audit persistence mode，也 MUST NOT 为 Function/Workflow 新增数据库或外部副作用旁路。动作仍须遵守被选 Function/Workflow 的既有合同；经过 Plugin/Capability 的调用 MUST 继续接受 Agent permission 与 Capability 鉴权。未知或无法应用的更新项 MUST 安全跳过并只输出静态 `error_kind`；原始更新 payload、用户内容和下游错误文本不得进入日志。

**权限模型**

- **FR-017**: main Agent 的 Hook 配置变更（增加/修改/删除）MUST 仅限 Super 角色操作。
- **FR-018**: 非 main Agent 的 Hook 配置变更 MUST 限 System 及以上角色操作。

**快照与缓存一致性**

- **FR-019**: Orchestrator MUST 在每个 hop 调用 `fetch_content` 获取当前 Agent 内容；成功后 MUST 为该 hop 固定 `AgentContent.hooks` 快照，下一 hop MUST 重新获取并在成功后替换。后续加载失败 MUST 使用最近一次成功快照触发 `on_agent_error`，首次加载失败且无快照时 MUST 跳过。
- **FR-020**: Hook 创建、更新或删除成功后，系统 MUST 立即尝试失效对应 `agent:content:{id}` 缓存。失效失败 MUST 保持已提交的 Hook 变更成功，并且 MUST 仅记录静态 `error_kind` tracing，不得记录 Redis 原始错误文本。

### Key Entities *(include if feature involves data)*

- **AgentHook**：Agent 与 Hook 的一对多关联实体。核心属性：Agent 引用、名称、描述、触发点（枚举）、动作类型（枚举）、动作参数（JSON）、是否启用、排序序号、创建/更新时间。

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 管理员可在 2 分钟内完成为一个 Agent 配置一个 Hook 的全流程操作（选择触发点 → 选择动作 → 填写参数 → 保存）。
- **SC-002**: Hook 配置保存成功后无需重启；对应 Agent 内容缓存立即被请求失效，下一次成功的 `fetch_content`（包括同一会话的下一 hop）即按新配置固定 Hook 快照。缓存失效异常不得把已成功的 Hook 变更改写为失败。
- **SC-003**: 单次 Hook 执行（含 Function 调用）的调度开销 p95 ≤ 50 毫秒（不含 Hook 动作本身的执行耗时）。
- **SC-004**: HTTP Webhook 调用超时前的等待时间可配置且实际行为与配置偏差不超过 ±1 秒。
- **SC-005**: Hook 执行失败时 100% 产生结构化失败事件，不出现无日志的失败情况。
- **SC-006**: 任意数量的 Hook 执行及 Webhook 重试均不会新增 Hook 执行历史数据库记录。
- **SC-007**: Agent 主流程因 Hook 阻塞模式失败而中止时，`POST /api/assistant` 100% 返回同步 JSON 错误：超时为 6004/HTTP 408，其他阻塞失败为 6005/HTTP 500；消息不得嵌套 JSON。
- **SC-008**: 每个成功 hop 快照之后的终止 Agent 错误 100% 进入统一 `on_agent_error` 阶段；后续加载失败使用最近成功快照，首次加载失败无快照时明确跳过。该阶段最多持续 30 秒，失败或预算耗尽时最终同步 JSON 的错误码和消息与原始错误完全一致。
- **SC-009**: 自动化测试 MUST 证明普通 Assistant 上下文的 runtime audit 可进入 best-effort DB 队列且携带 request/session 关联 ID；同一上下文经 `for_hook()` 后以及再次派生时均保持相同关联 ID、DB 入队数为 0，并且显式接线覆盖 Hook → Workflow → Plugin → Capability，不依赖 task-local 隐式状态。

## Assumptions

- Hook 的运行时执行能力（Function/Workflow 调用、HTTP 请求）复用 004-agent-runtime 已有的 capability dispatcher 和基础设施，不引入新的执行引擎。
- Hook 配置存储使用现有 MySQL 数据库，通过新增表实现，沿用现有 migration 机制。
- HTTP Webhook 的 SSRF 防护复用 FR-001 中已实现的 URL 安全校验逻辑（内网 IP + 云元数据端点拒绝）。
- Hook 执行不引入新的 Role-Based Access Control 角色，沿用 003-admin-center 的 Super / System / Normal 三级权限模型。
- 管理员理解基础的事件驱动概念（触发点=事件，动作=响应），无需 Hook 编程教程。
- Hook 配置的 UI 更改集成到现有 Agent 编辑页面中，作为新增的配置区域（Tab 页或折叠面板）。
- 内置 Function 集合（`format_template`、`json_parse`、`json_stringify`、`text_regex_match`、`chat_respond`）中，`chat_respond` 不可被 Hook 调用（因其专门用于 Agent 对话回复，被 Hook 误用可能导致混淆）。
- **Hook 使用受控运行时更新**：Function/Workflow 输入合并 `action_params.args`，随后由运行时覆盖注入只读 `_agent_context` 快照；只有输出 `_agent_context_updates` 会由运行时映射到当前内存 `AgentContext` 的 records、extensions 与字符串 metadata。该保留键不得进入 Workflow 公共 end-output 或 output schema。此通道不是持久化 Agent 配置或数据库写入 API；若需修改 system prompt，仍应通过 Agent 编辑页面直接编辑。
- **Hook 执行只使用 tracing**：运行时不创建新的 Hook 执行历史数据；运维排障通过集中式日志平台按结构化字段检索。
- **执行上下文只控制审计副本**：`RuntimeExecutionContext` 的 tracing-only / best-effort DB 模式只决定经过 runtime audit 管道的脱敏事件是否可以排队写入 `runtime_audit_logs`；它不代表禁用 Function/Workflow 业务逻辑或配置读取所需的数据库访问。
- **已有历史数据不在本次范围内**：已部署环境中的遗留执行历史表和历史数据保持不动，本次变更不执行删除或迁移。
