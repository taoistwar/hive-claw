# Feature Specification: 对外 Assistant API

**Feature Branch**: `007-external-assistant-api`  
**Created**: 2026-06-01  
**Status**: Draft  
**Input**: User description: "添加一个对外API assistant，不需要认证。输入参数是JSON，有两个字段：user_id和message。先使用一个外部只读数据库中校验用户是否存在，校验会员等级与访问次数，同步用户到内部数据库，调用agent处理消息并返回结果。"

## Clarifications

### Session 2026-06-01

- Q: 公开 API 是否需要额外的滥用防护措施？ → A: 使用签名鉴权 — 调用方使用预共享 secret 对请求签名，签名算法为 MD5(secret + requestURI + "?body=" + body)，Content-Type 为 `application/json; charset=UTF-8`。
- Q: 当 `message` 为空字符串时，系统应如何处理？ → A: 拒绝请求，返回参数校验错误。
- Q: `effective_end_time` 恰好等于当前时间时，是有效还是已过期？ → A: 包含边界，`effective_end_time >= 当前时间` 视为有效。
- Q: LLM 调用超时或返回错误时，API 应如何响应？ → A: 返回明确的服务繁忙提示，不消耗用户配额。
- Q: `user_id` 为 ≤ 0 时如何处理？ → A: 拒绝请求，返回参数校验错误，不查询外部数据库。
- Q: Assistant API 应使用哪个 Agent 来处理消息？ → A: 使用数据库 `agents` 表中的 Main Agent（默认主智能体），包含其 system_prompt、关联 tools、skills 完整配置，由 `crates/agent` 的 `AgentRunner` 执行编排。

## User Scenarios & Testing *(mandatory)*

### User Story 1 - 已注册用户发起AI对话 (Priority: P1)

外部系统（如网站、App、微信小程序）通过该 API 以用户身份发起一次 AI 对话。系统验证用户合法性后，使用 AI 助手处理消息并返回回复。

**Why this priority**: 这是整个 API 的核心功能，没有它 API 无任何价值。

**Independent Test**: 提供一个合法的 `user_id` 和一条 `message`，验证返回包含 AI 回复的成功响应。

**Acceptance Scenarios**:

1. **Given** 外部数据库中存在该 `user_id` 且为普通用户、当日访问次数未超限，**When** 发送 `{ "user_id": 123, "message": "今天天气怎么样" }`，**Then** 返回 `{ "success": true, "reply": "...", "message": "ok" }` 且包含有效的 AI 回复内容。
2. **Given** 外部数据库中存在该 `user_id` 且为 VIP 会员在有效期内，**When** 发送消息，**Then** 按 VIP 每日配额（`vip_ask_times` 配置，默认50次）进行限流。
3. **Given** 外部数据库中存在该 `user_id` 但无有效会员，**When** 发送消息，**Then** 按普通用户每日配额（`normal_ask_times` 配置，默认5次）进行限流。

---

### User Story 2 - 非法或不存在用户被拒绝 (Priority: P1)

当 `user_id` 在外部用户表中不存在时，API 直接拒绝请求，不消耗任何配额。

**Why this priority**: 安全边界——必须在处理任何业务逻辑之前验证调用方身份合法性。

**Independent Test**: 使用一个不存在于外部 `cloud_user` 表的 `user_id`，验证返回 `{ "success": false, "message": "User not found" }`。

**Acceptance Scenarios**:

1. **Given** 外部 `cloud_user` 表中无该 `user_id`，**When** 发送请求，**Then** 返回 `success: false`，`message` 为 "User not found"，不执行任何后续逻辑。

---

### User Story 3 - 每日访问次数限流 (Priority: P2)

系统根据用户会员等级限制每日访问次数，超过配额后当天无法继续访问。

**Why this priority**: 防止 API 被滥用，保护系统资源，同时利用会员等级实现差异化服务。

**Independent Test**: 模拟当天已用完配额的普通用户，再次发送请求，验证返回限额提示。

**Acceptance Scenarios**:

1. **Given** 普通用户当日已访问 5 次，**When** 第 6 次发送请求，**Then** 返回 `success: false`，提示已达到当日限额。
2. **Given** VIP 用户当日已访问 50 次，**When** 第 51 次发送请求，**Then** 返回 `success: false`，提示已达到当日限额。
3. **Given** 用户第一次在当天访问，**When** 发送请求成功，**Then** 该用户的每日计数器重置清零，并在 24 小时后自动过期。
4. **Given** Redis 不可用，**When** 发送请求，**Then** 返回明确的错误信息 "Redis unavailable"，不继续处理。

---

### User Story 4 - 新用户自动同步到内部系统 (Priority: P2)

首次通过 API 访问的用户，如果尚不存在于内部 `users` 表，系统自动创建用户记录。

**Why this priority**: 确保外部用户能够无缝使用内部系统能力，同时为后续扩展（如用户画像、使用统计）奠定基础。

**Independent Test**: 使用一个在外部数据库存在但内部 `users` 表中不存在的 `user_id`，验证请求成功后内部 `users` 表新增了一条记录。

**Acceptance Scenarios**:

1. **Given** 外部用户存在但内部 `users` 表中无记录，**When** 首次发送请求，**Then** 系统自动在内部 `users` 表创建该用户（密码为默认值），请求正常处理。
2. **Given** 外部用户存在且内部 `users` 表已有记录，**When** 发送请求，**Then** 跳过创建步骤，直接处理消息。

---

### User Story 5 - 外部服务不可用时的降级处理 (Priority: P3)

当外部只读数据库连接不可用或API服务未配置外部数据库时，API 返回明确的服务不可用信息。

**Why this priority**: 确保生产环境运维人员能够快速识别问题根因，而非收到模糊的系统错误。

**Independent Test**: 不配置外部数据库连接，发送请求，验证返回 `"Assistant service unavailable"`。

**Acceptance Scenarios**:

1. **Given** 外部数据库连接未配置，**When** 发送请求，**Then** 返回 `success: false`，`message` 为 "Assistant service unavailable"。
2. **Given** 外部数据库连接已配置但连接失败，**When** 发送请求，**Then** 返回 `success: false` 并提示服务不可用，不影响主系统稳定运行。

---

### Edge Cases

- 用户在 23:59:59 访问后，00:00:00 再次访问 — 计数器是否按自然天正确重置？
- `message` 为空字符串时 → 返回参数校验错误，不消耗任何配额。
- 同一用户短时间内高并发请求（如同时发送 3 条）— 限流计数是否准确？
- 外部数据库中 `cc_user_membership` 表的 `effective_end_time` 恰好等于当前时间 → 视为有效（`>=`），当天全天可享受 VIP 权益。
- `user_id` 为负数或零 → 参数校验阶段直接拒绝，不查询外部数据库。
- LLM 调用超时或返回错误 → 不消耗配额，返回"服务繁忙"提示。

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: 系统 MUST 提供 `POST /api/assistant` 端点，通过请求签名鉴权（`sign` 参数）替代传统身份认证。
- **FR-002**: 请求体 MUST 接受 JSON 格式，包含 `user_id`（整数）和 `message`（字符串）两个必填字段。
- **FR-003**: 系统 MUST 通过外部只读数据库（`cloud_computer`）的 `cloud_user` 表校验 `user_id` 是否存在。
- **FR-004**: 当 `user_id` 在外部数据库中不存在时，系统 MUST 立即返回失败响应，不执行后续处理。
- **FR-005**: 系统 MUST 通过外部数据库的 `cc_user_membership` 表查询用户会员等级，根据 `membership_level` 和 `effective_end_time` 判断会员是否有效。
- **FR-006**: VIP 会员（有有效会员记录）每日访问上限 MUST 从全局配置 `vip_ask_times` 读取（默认 50 次）。
- **FR-007**: 普通用户（无有效会员记录）每日访问上限 MUST 从全局配置 `normal_ask_times` 读取（默认 5 次）。
- **FR-008**: 系统 MUST 使用 Redis 记录每日访问次数，Key 格式为 `assistant:daily:{user_id}`，首次创建时设置 24 小时过期。
- **FR-009**: 当访问次数超过配额时，系统 MUST 返回失败响应并说明已达每日限额。
- **FR-010**: 系统 MUST 检查内部 `users` 表是否存在该 `user_id`，不存在时自动创建用户记录（`phone` 为 `assistant_{user_id}`，密码为固定默认值）。
- **FR-011**: 系统 MUST 使用 `agents` 表中的 Main Agent（主智能体）处理 `message`，由 `crates/agent` 的 `AgentRunner` 执行编排（含 system_prompt、关联 tools、skills、LLM 调用），并返回最终结果文本。Agent 执行成功定义为：在最多 5 轮迭代内返回 `final_content`（不含 tool error 导致的 stop）；超时或多轮耗尽视为失败。
- **FR-012**: 系统 MUST 在外部数据库不可用或未配置时返回明确的服务不可用信息，不影响主系统稳定性。
- **FR-013**: 系统 MUST 提供统一的 JSON 响应格式，包含 `success`（布尔）、`reply`（可选字符串）、`message`（字符串）字段。
- **FR-014**: 系统 MUST 校验请求签名 — 对请求 URL 路径 + 请求体，使用预共享的 `secret` 计算 MD5 签名，与请求中的 `sign` 参数比对；签名不匹配时拒绝请求。当环境变量 `ASSISTANT_SECRET` 未配置或为空时，系统跳过签名校验（适用于开发/测试环境）。
- **FR-015**: 请求 Content-Type MUST 为 `application/json; charset=UTF-8`，签名算法为 `MD5(secret + requestURI + "?body=" + body)`，其中 `body` 为请求体的原始 JSON 字符串。
- **FR-016**: 系统 MUST 校验 `message` 不得为空字符串，为空时返回参数校验错误，不消耗用户配额。
- **FR-017**: LLM 调用超时或失败时，系统 MUST 返回明确的服务繁忙提示（如"服务繁忙，请稍后重试"），且不消耗该次请求的用户每日配额。
- **FR-018**: 系统 MUST 校验 `user_id` 必须为正整数（>0），≤0 时返回参数校验错误，不查询外部数据库。
- **FR-019**: 全局配置 `vip_ask_times` / `normal_ask_times` 读取失败时，系统 MUST 使用硬编码默认值（50 / 5）作为兜底，不中断请求处理。
- **FR-020**: Main Agent（`agents` 表 id=1）不存在时，系统 MUST 返回服务不可用错误，不执行后续处理。

### Key Entities

- **AssistantRequest**: 对外 API 的请求体，包含 `user_id`（调用方用户标识）和 `message`（用户输入）。
- **AssistantResponse**: 对外 API 的响应体，包含 `success`（是否成功）、`reply`（AI 回复内容，仅成功时返回）、`message`（状态描述）。
- **CloudUser**（外部表 `cloud_user`）: 外部系统中的用户基础表，以 `ID` 作为用户唯一标识。
- **CcUserMembership**（外部表 `cc_user_membership`）: 外部系统中的会员记录表，包含 `id`（用户 ID）、`membership_level`（会员等级）、`effective_end_time`（有效期截止时间）。
- **内部 User**（表 `users`）: 内部系统用户表，对外 API 新用户会自动同步到该表。
- **GlobalConfig**: 全局配置项，`vip_ask_times` 和 `normal_ask_times` 分别控制 VIP 和普通用户的每日访问上限。
- **日访问计数器**（Redis）: 以 `assistant:daily:{user_id}` 为 Key 的整数值，记录当天已使用次数，24 小时后自动过期。
- **预共享 Secret**: 调用方与服务端约定好的密钥字符串，用于生成和校验请求签名。通过环境变量注入，不在 API 请求中传输。
- **Main Agent**（内部表 `agents`）: 系统中的默认主智能体，包含 system_prompt、model_preset、关联的 tools 和 skills 配置，由 `crates/agent` 的 `AgentRunner` 加载并执行多轮 LLM 编排。

### Access Control

- **签名鉴权**: 调用方使用预共享的 `secret` 对每个请求生成 `sign` 参数，服务端校验通过后方可处理。不要求 token 或 session。
- **Content-Type**: 请求报文必须为 `application/json; charset=UTF-8`。

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 合法用户发送有效请求后，在 10 秒内获得 AI 回复结果。
- **SC-002**: 不存在于外部数据库的 `user_id` 请求在 1 秒内被拒绝并返回明确错误信息。
- **SC-003**: 每日访问限流在 Redis 正常运行时 100% 准确执行，不出现超额访问（Redis INCR 为原子操作；回滚失败等极端情况导致的最大误差为 +1 次/天，属于可接受范围）。
- **SC-004**: 外部数据库连接失败时，系统在 1 秒内返回服务不可用提示，主系统其余功能不受影响。
- **SC-005**: 新用户首次访问时，自动同步到内部系统的过程对调用方完全透明（响应时间不受影响，无需额外交互），同步成功或失败均不影响该次请求的正常处理。
- **SC-006**: API 在并发 100 QPS 的压力下，成功响应率达到 95% 以上。

## Assumptions

- 外部只读数据库（`43.140.221.181:3306/cloud_computer`）在部署环境中网络可达。
- 外部 `cloud_user` 表的 `ID` 字段为自增主键或唯一标识，与 `user_id` 参数一一对应。
- 外部 `cc_user_membership` 表的 `id` 字段与 `cloud_user.ID` 对应，两者为 1:1 或 1:N 关系（取第一条有效记录）。
- `cc_user_membership.effective_end_time` 用于判断会员是否在有效期内，大于等于当前时间即为有效（当天全天有效）。
- 内部 LLM（大语言模型）服务已正常配置并可通过默认 preset 调用。
- Redis 在部署环境中已配置并可正常访问。
- 每日访问次数以自然天为统计周期，使用 24 小时 TTL 实现。
- 内部 `users` 表的 `id` 字段支持指定值插入（非全自增），以便与外部用户 ID 保持一致。
- `message` 字段长度无硬性上限，由 Agent/LLM 的上下文窗口自然约束。
- 全局配置 `vip_ask_times` 和 `normal_ask_times` 已通过管理中心预先配置好默认值。
- 对外 API 使用与 Web 端用户聊天相同的 Main Agent 配置（system_prompt、tools、skills、model_preset），由 `crates/agent` 的 `AgentRunner` 执行编排，但对外 API 无持久化会话（Session）概念，每次请求独立处理。
- MD5 签名鉴权用于请求来源验证，非加密目的。已知 MD5 碰撞风险，在签名上下文下重放攻击为主要威胁（v1 接受此风险，后续版本可升级签名算法）。
- 当前版本不实现请求重放保护（如 nonce/timestamp），依赖 HTTPS 传输层安全 + 预共享密钥的机密性。
- 外部数据库连接通过 `EXTERNAL_DB_URL` 环境变量注入，凭证不在代码或日志中暴露；连接使用只读权限（仅 SELECT 查询），部署时确保内网可达免 TLS。
- API 级 DoS 防护（如 IP 限流）不属于 v1 范围，当前仅实现用户级每日配额控制。
- 外部数据库返回异常数据时（多余行、编码问题、NULL 列），查询层通过 `fetch_optional` / `LIMIT 1` / `unwrap_or` 安全兜底。
- 外部数据库连接池限制为 5 个只读连接（部署时按需调整），避免连接耗尽影响主系统。
- SC-006 的 100 QPS 吞吐量目标面向整体端到端，不单独分解到各依赖（外部 DB、Redis、LLM），负载测试在部署后验证。
- 冷启动延迟（首次请求触发 AgentRunner 初始化、DB 连接池 warm-up）不计入 SC-001 的 10 秒超时约束。
- 请求关联 ID（request_id）由 hiveweb 现有 middleware 注入，assistant 模块无需单独实现。
- AgentRunner 内部错误通过 `AgentRunResult.error` 字段返回结构化信息，宿主层可据此记录日志。
- Metrics 和告警阈值（QPS、错误率、延迟分布）属于部署运维层关注点，不在 v1 规格中定义。
