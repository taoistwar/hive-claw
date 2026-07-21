# Feature Specification: Agent Hook 配置管理

**Feature Branch**: `008-agent-hook-config`
**Created**: 2026-06-02
**Status**: Draft
**Input**: User description: "在管理中心为Agent配置hook"

## Clarifications

### Session 2026-06-02

- Q: 每个 Agent 最多允许配置多少个 Hook？ → A: 每个触发点最多 5 个 Hook，总 Hook 数不限（由各触发点独立限制）
- Q: Webhook 调用失败后是否需要自动重试？ → A: 后台异步重试最多 3 次（间隔指数退避），不阻塞 Hook 串行执行流；最终结果输出结构化 tracing（该记录方式由 2026-07-16 决议明确）
- Q: 非阻塞模式 Hook 失败时，后续 Hook 应继续还是跳过？ → A: 始终继续执行后续 Hook（非阻塞模式下各 Hook 相互独立，单点失败不影响后续）
- Q: Hook 执行结果是否应注入 Agent 的 system_prompt 或修改运行时状态？ → A: 否。Hook 为只读观察者，不修改 Agent 状态（当时采用的持久化审计方案已由 2026-07-16 决议取代）

### Session 2026-07-16

- Q: Hook 执行记录是否继续持久化到数据库？ → A: 否。为避免执行量增长导致数据库容量压力，Hook 执行结果仅输出结构化 tracing；不再持久化执行历史，也不再提供执行历史查询界面或接口。已有历史表和历史数据保持不动。

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

---

### User Story 2 - 通过 HTTP Webhook 与外部系统集成 (Priority: P2)

管理员配置 Hook 动作为 HTTP 回调（Webhook），在 Agent 执行的关键节点向外部系统发送事件通知。

**Why this priority**: HTTP Webhook 是 Agent 与企业现有系统（如监控告警、工单系统、数据管道）集成的关键通道，扩展了 Agent 的生态连接能力。

**Independent Test**: 为 Agent 配置 `after_agent_end` Hook（动作=HTTP Webhook，URL 指向测试端点）→ 触发 Agent 对话 → 验证外部端点收到包含 Agent 执行摘要的 HTTP POST 请求。

**Acceptance Scenarios**:

1. **Given** 管理员配置了一个 HTTP Webhook 类型的 Hook, **When** 填写回调 URL、可选的请求头和超时时间, **Then** Hook 配置保存成功。
2. **Given** HTTP Webhook Hook 已配置, **When** Hook 触发时, **Then** 系统向目标 URL 发送 POST 请求，请求体包含触发上下文（Agent 标识、会话 ID、触发事件类型、时间戳）。
3. **Given** HTTP Webhook 的目标 URL 不可达或超时, **When** Hook 触发, **Then** 系统输出脱敏的结构化失败事件，不阻塞 Agent 主流程继续执行。
4. **Given** 管理员配置 Webhook, **When** 填写 URL, **Then** 系统校验 URL 格式必须为 `https://` 协议（拒绝 `http://`），且不在 SSRF 黑名单中。

---

### User Story 3 - 通过 Hook 调用 Workflow/Function 实现自动化 (Priority: P2)

管理员配置 Hook 动作为调用已有的 Workflow 或 Function，实现 Agent 执行前后的自动化处理（如对话前记录审计日志、对话后触发数据处理流水线）。Hook 作为**只读观察者**执行，不修改 Agent 的 system_prompt 或运行时状态。

**Why this priority**: 复用平台已有的 Function/Workflow 资产，使 Hook 能执行复杂业务逻辑，最大化现有投资价值。

**Independent Test**: 配置 `before_agent_start` Hook（动作=调用 Function `format_template`）→ 触发 Agent 对话 → 验证 Hook 执行成功、产生结构化成功事件，并且数据库没有新增 Hook 执行记录。

**Acceptance Scenarios**:

1. **Given** 管理员选择 `call_function` 动作类型的 Hook, **When** 从 Function 列表中选择目标 Function 并填写入参映射, **Then** Hook 配置保存成功，参数通过 JSON Schema 校验。
2. **Given** 管理员选择 `call_workflow` 动作类型的 Hook, **When** 选择目标 Workflow 并配置入参来源（可从触发上下文中提取）, **Then** Hook 配置保存成功。
3. **Given** Hook 调用的 Function 执行失败, **When** Hook 触发, **Then** 错误被记录为脱敏的结构化事件，Hook 失败不阻止 Agent 主流程（除非 Hook 配置为"阻塞模式"）。

---

### Edge Cases

- **Hook 执行超时**：单个 Hook 执行超时（默认 10 秒，可配置），系统强制中止并输出结构化超时事件，不阻塞 Agent 主流程。HTTP Webhook 超时不计入重试窗口（每次重试独立计时）。
- **Webhook 重试**：HTTP Webhook 失败后进入后台重试队列（最多 3 次，间隔 1s/2s/4s 指数退避）。重试期间后续 Hook 照常执行。每次重试及最终结果均输出结构化事件，但不写入数据库。
- **Hook 配置更新时机**：Hook 配置保存后立即生效；已在进行中的 Agent 会话使用会话开始时的 Hook 快照，不受后续配置变更影响。
- **多个 Hook 顺序**：同一触发点可配置多个 Hook，按其配置序号依次串行执行。非阻塞模式下各 Hook 相互独立，单点失败不阻止后续 Hook。阻塞模式下失败立即中止。
- **Hook 动作引用失效**：Hook 引用的 Function/Workflow 被删除时，Hook 配置状态变为"失效"，触发时跳过并输出结构化警告事件。
- **循环引用检测**：Hook 触发的 Function/Workflow 如果携带可能导致递归触发 Agent 的操作，系统在配置保存时不检测（运行时由 Agent 最大跳次防护兜底）。
- **main Agent Hook 变更权限**：与现有 Admin Center 权限模型一致——非 Super 角色无法修改 main Agent 的任何配置（含 Hook），UI 中 Hook 配置区域不渲染。
- **并发执行**：同一 Agent 的多个 Hook 按配置顺序串行执行，避免并发带来的状态管理复杂性。
- **Hook 数量上限**：每个触发点最多配置 5 个 Hook。保存时若超出上限，系统拒绝并提示"该触发点最多配置 5 个 Hook"。
- **on_agent_error 提前失败路径**：若 Agent 执行在 `build_agent_context()` 阶段即失败（如 DB 连接错误），此时 hooks 尚未加载，`on_agent_error` Hook 跳过执行。此场景仅输出原始错误事件，不追加 Hook 执行事件。

## Requirements *(mandatory)*

### Functional Requirements

**Hook 配置管理**

- **FR-001**: 系统 MUST 支持为每个 Agent 配置零个或多个 Hook，每个 Hook 定义触发点（trigger_point）和动作（action）。每个触发点最多 5 个 Hook。
- **FR-002**: Hook 触发点 MUST 覆盖以下 Agent 生命周期节点：
  - `before_agent_start`：Agent 开始执行前触发（在 system_prompt 组装之后、LLM 首次调用之前）
  - `after_agent_end`：Agent 执行结束后触发（在最终消息持久化之后、SSE `done` 事件发出之前）
  - `on_agent_error`：Agent 执行出错时触发（在原始错误事件输出之后、SSE `error` 事件发出之前）
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
  - 拒绝内网 IP 和云元数据端点（复用现有 SSRF 防护规则）
- **FR-007b**: `action_params.headers` 中的自定义 HTTP header 键值对 MUST 校验：拒绝包含 `\r` 或 `\n` 的 header name/value（防止 HTTP header injection）。header name 仅允许 `[a-zA-Z0-9_-]+` 字符集。
- **FR-007a**: HTTP Webhook 调用失败时，系统 MUST 进入后台异步重试队列：最多重试 3 次（间隔指数退避：1s → 2s → 4s），重试不阻塞当前 Hook 串行执行流（后续 Hook 照常执行）。每次重试和最终 error 状态 MUST 输出结构化事件，不得持久化执行记录。

**Hook 执行**

- **FR-008**: 当 Hook 触发时，系统 MUST 向 Hook 动作传递触发上下文（Agent 标识、会话 ID、触发事件、时间戳）。
- **FR-009**: Hook 执行 MUST 不阻塞 Agent 主流程——默认采用"异步"模式，Hook 失败仅输出结构化事件，不影响 Agent 继续执行。
- **FR-010**: 系统 MUST 支持 Hook 的"阻塞模式"配置项。当设置为阻塞模式时，Hook 失败将中止 Agent 当前流程并返回错误。
- **FR-011**: 单个 Hook 执行超时时间 MUST 可配置（默认 10 秒），超时后强制中止并输出结构化超时事件。
- **FR-012**: 同一触发点的多个 Hook MUST 按排序序号从小到大依次串行执行。非阻塞模式下，单个 Hook 失败不影响后续 Hook 继续执行（各 Hook 相互独立）。阻塞模式下，单个 Hook 失败立即中止 Agent 流程，不执行后续 Hook。
- **FR-013**: Hook 执行时的 capability 鉴权 MUST 沿用当前执行中 Agent 的 permissions 集合。

**审计与可观测性**

- **FR-014**: 每次 Hook 触发执行 MUST 输出结构化 tracing 事件，至少包含：Agent ID、Hook ID、触发事件、动作类型、执行结果（success/error/timeout/skipped）、耗时、请求 ID。
- **FR-015**: 系统 MUST NOT 将 Hook 执行结果、上下文快照或重试结果持久化到数据库，也 MUST NOT 提供 Hook 执行历史查询接口或管理界面。
- **FR-016**: Hook 执行失败时，结构化事件 MUST 只记录白名单错误分类（`error_kind`），不得记录下游返回的任意错误文本、完整用户消息正文、Webhook URL、请求体或响应体。

**权限模型**

- **FR-017**: main Agent 的 Hook 配置变更（增加/修改/删除）MUST 仅限 Super 角色操作。
- **FR-018**: 非 main Agent 的 Hook 配置变更 MUST 限 System 及以上角色操作。

### Key Entities *(include if feature involves data)*

- **AgentHook**：Agent 与 Hook 的一对多关联实体。核心属性：Agent 引用、名称、描述、触发点（枚举）、动作类型（枚举）、动作参数（JSON）、是否启用、排序序号、创建/更新时间。

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 管理员可在 2 分钟内完成为一个 Agent 配置一个 Hook 的全流程操作（选择触发点 → 选择动作 → 填写参数 → 保存）。
- **SC-002**: Hook 配置保存后立即生效，下一个 Agent 会话即按新配置触发 Hook（无重启需求）。
- **SC-003**: 单次 Hook 执行（含 Function 调用）的调度开销 p95 ≤ 50 毫秒（不含 Hook 动作本身的执行耗时）。
- **SC-004**: HTTP Webhook 调用超时前的等待时间可配置且实际行为与配置偏差不超过 ±1 秒。
- **SC-005**: Hook 执行失败时 100% 产生结构化失败事件，不出现无日志的失败情况。
- **SC-006**: 任意数量的 Hook 执行及 Webhook 重试均不会新增 Hook 执行历史数据库记录。
- **SC-007**: Agent 主流程因 Hook 阻塞模式失败而中止时，SSE 流 100% 发出 `error` 事件且含明确错误码。

## Assumptions

- Hook 的运行时执行能力（Function/Workflow 调用、HTTP 请求）复用 004-agent-runtime 已有的 capability dispatcher 和基础设施，不引入新的执行引擎。
- Hook 配置存储使用现有 MySQL 数据库，通过新增表实现，沿用现有 migration 机制。
- HTTP Webhook 的 SSRF 防护复用 FR-001 中已实现的 URL 安全校验逻辑（内网 IP + 云元数据端点拒绝）。
- Hook 执行不引入新的 Role-Based Access Control 角色，沿用 003-admin-center 的 Super / System / Normal 三级权限模型。
- 管理员理解基础的事件驱动概念（触发点=事件，动作=响应），无需 Hook 编程教程。
- Hook 配置的 UI 更改集成到现有 Agent 编辑页面中，作为新增的配置区域（Tab 页或折叠面板）。
- 内置 Function 集合（`format_template`、`json_parse`、`json_stringify`、`text_regex_match`、`chat_respond`）中，`chat_respond` 不可被 Hook 调用（因其专门用于 Agent 对话回复，被 Hook 误用可能导致混淆）。
- **Hook 为只读观察者**：Hook 执行结果不注入到 Agent 的 system_prompt 或修改 Agent 运行时状态。若需修改 system_prompt，应通过 Agent 编辑页面直接编辑。
- **Hook 执行只使用 tracing**：运行时不创建新的 Hook 执行历史数据；运维排障通过集中式日志平台按结构化字段检索。
- **已有历史数据不在本次范围内**：已部署环境中的遗留执行历史表和历史数据保持不动，本次变更不执行删除或迁移。
