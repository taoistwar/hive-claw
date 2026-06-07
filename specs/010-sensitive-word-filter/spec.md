# Feature Specification: 敏感词过滤

**Feature Branch**: `260516-sensitive-word-filter`  
**Created**: 2026-06-07  
**Status**: Draft  
**Input**: User description: "为对外接口 `/api/assistant` 添加敏感词过滤功能，如果用户的输入和Agent的输出有敏感词，直接返回给用户并友好提示。"

## Clarifications

### Session 2026-06-07

- Q: 输入被敏感词拦截时，API 应返回什么 HTTP 状态码和响应体格式？ → A: HTTP 200，响应体为 `{"reply": "友好提示文本", "filtered": true}`，与正常成功响应格式兼容
- Q: 管理员输入无效正则表达式时如何处理？ → A: 保存时校验正则合法性，非法正则拒绝保存并提示错误信息
- Q: Agent 输出被敏感词替换时的响应格式？ → A: 与输入拦截一致，统一使用 `{"reply": "友好提示文本", "filtered": true}`
- Q: 预置敏感词库的数据来源？ → A: 混合多个开源来源（funNLP + houbb/sensitive-word 等），去重合并后作为 seed 数据预置
- Q: 预置词库与管理员 CRUD 的关系？ → A: 预置词与自定义词平等对待，管理员可自由启用/禁用/删除任何词条

## User Scenarios & Testing *(mandatory)*

### User Story 1 - 输入包含敏感词时拦截 (Priority: P1)

用户通过 `/api/assistant` 发送消息时，如果消息内容包含敏感词，系统在进入 AI 处理流程之前直接拒绝请求，返回友好提示，告知用户消息中包含不适宜内容。

**Why this priority**: 输入拦截是防御第一关，避免敏感内容进入 AI 系统，节省计算资源并降低合规风险。这是最基础、最高频的场景。

**Independent Test**: 模拟发送包含已知敏感词的用户消息，验证接口立即返回拦截响应而非进入 Agent 处理流程。

**Acceptance Scenarios**:

1. **Given** 已配置敏感词库，**When** 用户发送包含敏感词的消息，**Then** 系统返回拒绝响应，包含友好提示信息，且不创建会话、不调用 LLM
2. **Given** 已配置敏感词库，**When** 用户发送不包含任何敏感词的正常消息，**Then** 系统正常进入 AI 处理流程，不受任何影响
3. **Given** 已配置敏感词库，**When** 用户消息中敏感词以变体形式出现（如全角、夹杂空格），**Then** 系统根据匹配规则决定拦截或放行

---

### User Story 2 - Agent 输出包含敏感词时替换 (Priority: P2)

Agent 处理完用户请求并生成回复后，如果回复内容包含敏感词，系统将输出替换为友好提示信息返回给用户，不暴露原始敏感内容。

**Why this priority**: 输出过滤是防御第二关。AI 模型可能生成意外的不适宜内容，需要在返回之前检查和替换。

**Independent Test**: 模拟 Agent 返回包含敏感词的输出，验证用户收到的响应是替换后的友好提示而非原始内容。

**Acceptance Scenarios**:

1. **Given** Agent 完成处理生成回复，**When** 回复内容包含敏感词，**Then** 用户收到的是友好提示，原始敏感内容不被返回
2. **Given** Agent 完成处理生成回复，**When** 回复内容不包含任何敏感词，**Then** 用户正常收到完整回复

---

### User Story 3 - 管理员管理敏感词库 (Priority: P3)

管理员可以通过管理后台维护敏感词列表，添加、删除、启用、禁用敏感词条目，修改后即时生效。

**Why this priority**: 敏感词库需要随业务需求变化而更新，管理功能是持续运营的基础，但可以在核心过滤功能上线后逐步完善。

**Independent Test**: 管理员登录后台，添加一条新敏感词，随后通过 `/api/assistant` 发送包含该敏感词的消息，验证被正确拦截。

**Acceptance Scenarios**:

1. **Given** 管理员登录管理后台，**When** 添加一条新的敏感词并启用，**Then** 该敏感词立即生效
2. **Given** 某敏感词已存在于词库中，**When** 管理员禁用该敏感词，**Then** 该词不再触发拦截
3. **Given** 多个敏感词并列存在，**When** 管理员删除某一条，**Then** 仅该条不再生效

---

### Edge Cases

- 空消息已在现有校验流程中处理，敏感词过滤在空消息校验之后执行
- 敏感词列表为空时：所有消息正常通过，不影响现有功能
- 极高频率并发请求下过滤互不影响
- Agent 输出为 extensions/content cards 类型时：仅检查文本内容部分
- 敏感词恰好被 URL 编码或 Unicode 转义后出现：在原始文本层面匹配

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: 系统 MUST 在 `/api/assistant` 接收到用户消息后、进入 Agent 处理之前，对 `message` 字段进行敏感词检查
- **FR-002**: 系统 MUST 在 Agent 生成回复后、返回给用户之前，对回复文本进行敏感词检查
- **FR-003**: 当用户输入匹配敏感词时，系统 MUST 拒绝请求并以 HTTP 200 + `{"reply": "<友好提示>", "filtered": true}` 格式返回，不进入 Agent 处理流程
- **FR-004**: 当 Agent 输出匹配敏感词时，系统 MUST 将回复的 `content` 字段替换为友好提示并附加 `"filtered": true` 标记，不暴露原始内容（响应格式与 contracts/api.md §2 一致）
- **FR-005**: 敏感词匹配 MUST 支持精确匹配（大小写不敏感）和正则表达式匹配两种模式。正则匹配需防范 ReDoS 攻击，限制回溯深度或使用超时机制。保存正则敏感词时 MUST 校验正则合法性，非法正则拒绝保存。
- **FR-006**: 敏感词列表 MUST 持久化存储，服务重启后保留
- **FR-007**: 管理员 MUST 能够通过管理后台 CRUD 敏感词条目（至少包含：敏感词文本、是否启用）
- **FR-008**: 敏感词变更 MUST 在无需重启服务的情况下生效
- **FR-009**: 输入被拦截时，系统 MUST 记录拦截日志（至少包含：用户 ID、触发词、时间戳）
- **FR-010**: 输出被替换时，系统 MUST 记录替换日志（至少包含：会话 ID、触发词、时间戳）
- **FR-011**: 敏感词过滤 MUST 不影响正常消息的处理性能（单次检查增加延迟 < 5ms）
- **FR-012**: 系统 MUST 在首次部署时预置一组基础敏感词（从多个开源中文敏感词库合并去重），通过 seed 脚本导入，管理员可自由编辑或删除

### Key Entities

- **SensitiveWord（敏感词条目）**: 表示词库中的一条记录，包含：文本内容、匹配模式（精确/正则）、是否启用、创建/更新时间
- **FilterLog（过滤日志）**: 表示一次过滤事件，包含：事件类型（输入拦截/输出替换）、触发词、用户/会话标识、时间戳

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 包含已知敏感词的用户消息被 100% 拦截
- **SC-002**: 不包含敏感词的正常消息被 100% 放行（无误拦）
- **SC-003**: 单次敏感词检查增加的延迟 < 5ms
- **SC-004**: 敏感词添加/修改后 1 秒内生效，无需重启服务
- **SC-005**: 管理后台敏感词增删改操作 2 步内完成

## Assumptions

- 敏感词数量级在 10,000 条以内（含预置词库），适用于内存缓存方案
- 预置词库来源于 funNLP、houbb/sensitive-word 等开源项目，通过 seed 脚本导入
- 友好提示文案为固定通用提示，后续可按需自定义
- 管理后台复用现有 web-admin 前端框架（React + Ant Design）
- 敏感词过滤仅针对 `/api/assistant` 接口，不影响内部 API
- 外部数据库校验（cloud_user、membership）在敏感词过滤之后执行
