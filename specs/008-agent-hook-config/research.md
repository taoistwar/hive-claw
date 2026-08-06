# Research: Agent Hook 配置管理

**Feature**: `008-agent-hook-config`
**Status**: Draft
**Phase**: 0 — Technical Decisions

---

## 1. Hook 配置与 AgentContent 的加载策略

**问题**: 每个 orchestration hop 如何加载当前 Agent 的 Hook 配置，并在配置变更后与既有 Agent 内容缓存保持一致？

**决策**: 每个 hop 调用 `services::agent::fetch_content(pool, redis, current_agent_id)`。缓存未命中时，`fetch_content_from_db()` 调用 `agent_hook::load_hooks_for_agent(pool, agent_id)` 加载所有 enabled Hook，按 `trigger_point` 分组并存入 `AgentContent.hooks`。Orchestrator 在成功返回后为当前 hop 克隆固定的 `LoadedAgentHooks`；下一 hop 再次 fetch 并在成功后替换，后续 fetch 失败使用最近一次成功快照，首次失败则无快照可执行。

**理由**:
- Hook 配置量小（每 Agent 最多 35 个），单次 JOIN 开销可忽略
- 复用已有 `agent:content:{id}` Redis 缓存，不为 Hook 引入独立缓存层
- Hook create/update/delete 成功后 best-effort 失效对应 Agent 内容缓存，使下一次 fetch 获取新配置
- 当前 hop 保持稳定快照，同时允许同一会话的下一 hop 观察到已保存的新配置
- 缓存失效失败不回滚已提交的 Hook 变更，仅记录固定 `redis_delete_failed` 分类；Redis miss、DB fallback 或 TTL 到期后自然收敛

**替代方案**: 为 Hook 单独新增 Redis 缓存 + TTL 刷新 — 被拒绝，因现有 AgentContent 缓存已覆盖该数据，额外缓存会引入双重失效问题。

---

## 2. Webhook 重试的持久化

**问题**: Webhook 后台重试期间的状态是否需要持久化到 DB？服务重启后重试是否会丢失？

**决策**: 不持久化重试状态或最终结果。重试通过 `tokio::spawn` 在内存中异步执行，重启后丢失未完成的重试；每次尝试和最终结果仅输出结构化 tracing。`HOOK_WEBHOOK_RETRY_MAX` 严格为 0..=3（默认/非法回退 3，0 禁用），退避从固定 1/2/4 秒表取值。

**理由**:
- Webhook 固定退避总和最多 7s（另加每次独立 attempt 预算），重启窗口内丢失概率低
- 引入重试状态表增加实现复杂度（Principle V: YAGNI）
- Webhook 本质是 best-effort 通知，不要求强可靠性

每个首次发送/重试 attempt 只有一个 Tokio timeout，future 从 DNS 解析开始，覆盖
全答案校验、DNS-pinned client 构造和 send。Webhook 不再同时受 `run_hooks`
相同 deadline 包裹，避免外层先取消导致首次 timeout 未安排 retry。Function/
Workflow 仍保留既有 `run_hooks` timeout。每触发点最多 5 个 Hook、前台串行与
每个 eligible transient Webhook failure 至多一个后台重试任务的既有语义不变。
共享 resolver 以 typed `Policy` / `ResolveUnavailable` 区分永久策略拒绝与暂时
DNS 不可用。Hook 的可重试集合封闭为 `ResolveUnavailable`、reqwest connection
error（`is_connect()`）与 attempt timeout；`Policy`、header、client build、
其他 Request 错误和 HTTP non-2xx 均不重试，禁止检查字符串决定重试。
- 若外部端点不可达超过 7s，大概率需人工介入而非自动恢复
- 避免高频 Hook 执行结果持续扩大数据库容量

**替代方案**: 在数据库中保存执行结果或重试状态并由 cron 恢复 — 被拒绝，因数据量增长、实现复杂度与可靠性收益不匹配。

---

## 3. Hook 执行的并发模型

**问题**: orchestrator 中同一触发点的多个 Hook 应串行还是并行执行？

**决策**: 严格串行，按 `sort_order` 升序依次执行。

**理由**:
- 避免并发带来的状态管理复杂性（共享 `AgentContext`、`pool` 引用）
- Hook 可通过受控 `_agent_context_updates` 更新当前上下文；严格串行使后续 Hook 读取到的快照和结构化 tracing 顺序保持确定
- 串行执行天然支持阻塞模式（失败即中止后续 Hook）
- 每个 Hook 执行时间短（默认超时 10s），串行带来的总延迟可控
- 与 spec FR-012 一致

**替代方案**: 同一触发点内 Hook 并行执行 `tokio::join!` — 被拒绝，因阻塞模式下需要协调中止逻辑，且并发 tracing 事件顺序不可预测。

---

## 4. Hook 超时与 Agent 超时的交互

**问题**: Hook 执行超时是否计入 Agent 整体超时预算？

**决策**: 不计入。Hook 超时独立计时（`tokio::time::timeout`），Agent 整体超时仅计算 LLM 调用 + tool 执行时间。

**终止错误阶段补充决策（2026-07-23）**: `on_agent_error` 仍不计入 Agent 正常执行预算，但该阶段自身具有固定 30 秒总预算。该外层预算覆盖阶段内全部 Hook，避免最多 5 个独立 Hook 超时串行累积；预算耗尽时取消剩余执行、记录脱敏 tracing，并保留原始终止错误。

**理由**:
- Hook 为可选的附加功能，不应惩罚未配置 Hook 的 Agent
- Hook 超时后强制中止该 Hook（`tokio::time::timeout` 的 `Elapsed` 错误）；非阻塞模式继续 Agent 主流程，阻塞模式中止请求并由 `POST /api/assistant` 返回 6004/HTTP 408
- 将 Hook 耗时计入 Agent 超时会导致超时行为不可预测（依赖 Hook 数量和复杂度）

---

## 5. Hook 配置的乐观锁

**问题**: Hook 更新/删除是否需要乐观锁？与 Agent 乐观锁分离还是共用同一版本号？

**决策**: Hook 表独立使用 `updated_at` 字段做乐观锁校验。不与 Agent 的乐观锁版本号共用。

**理由**:
- Hook 是独立的配置实体，与 Agent 配置的变更频率和触发者不同
- 共用 Agent 版本号会导致：修改 Hook 时 Agent 版本号变化，可能触发无关的 Agent 配置更新冲突
- `updated_at DATETIME(6)` 精度足够（微秒级），冲突概率低
- PUT 请求必须携带 `updated_at`，与当前值不匹配返回 4094

---

## 6. Hook 子链的显式执行上下文与审计边界

**问题**: 普通 Assistant runtime audit 可以 best-effort 写入数据库，而 2026-07-16 决议要求 Hook tracing-only。Hook 又可继续调用 Workflow、Plugin 和 Capability；如何在保留 request/session 关联 ID 的同时，防止任意嵌套层重新启用 DB audit？

**决策**:

- `POST /api/assistant` 在边界创建一个显式 `RuntimeExecutionContext::best_effort(Some(request_id), Some(session_id))`，并随 `OrchestratorDeps` 传入普通 Agent 调用树。
- 进入 Hook 时仅从父上下文调用 `for_hook()`。该操作保留 `request_id` 与 `session_id`，并把 runtime audit persistence mode 单向降级为 tracing-only。
- `HookDeps`、Workflow `ExecutorDeps`、Plugin `DispatchCtx`、Invoker 与 Capability Dispatcher 只显式传递或克隆该受限上下文。`for_hook()` 对 tracing-only 上下文重复调用仍为 tracing-only。
- persistence mode 保持 `RuntimeExecutionContext` 私有；类型不提供 `Default` 或 `Deserialize`，HTTP、Hook、Workflow 和 Plugin payload 都无法设置该模式。
- 普通 Assistant runtime audit 仍先输出 tracing，再通过有界队列 best-effort 入库；Hook 子链仅输出携带相同 request/session 关联 ID 的结构化 audit tracing，不进入 DB 队列。

**理由**:

- 显式参数传播使审计边界在代码审查中可见，也避免 spawned task 或执行器切换导致 task-local 状态丢失
- sticky 单向降级消除了 Hook → Workflow → Plugin → Capability 中任一层用普通构造器意外恢复 DB audit 的可能
- 关联 ID 与持久化模式同属一个不可由 payload 构造的值，避免日志链路相关性和审计策略发生漂移
- 保留普通 Assistant 的 best-effort DB audit，不扩大 2026-07-16 “Hook tracing-only”决议的范围

**边界说明**: tracing-only 只禁止 Hook 子链向 `runtime_audit_logs` 写入审计副本，不禁止 Hook 配置加载、Function/Workflow 元数据查询或业务动作本身所需的数据库访问。

**替代方案**:

- task-local audit flag — 被拒绝；异步 spawn、阻塞执行和嵌套执行器切换使传播边界不透明，也不利于源码契约验证。
- 在 Hook/Plugin payload 中增加 `tracing_only` 布尔值 — 被拒绝；不可信输入可以篡改审计策略。
- 每个子层重新构造上下文 — 被拒绝；容易丢失 request/session 关联 ID 并将 tracing-only 意外升级为 best-effort DB。

---

## 7. Hook 动作的受控 `AgentContext` 更新

**问题**: 早期澄清把 Hook 定义为严格只读观察者，但实际 Function/Workflow 集成已使用 `_agent_context` / `_agent_context_updates` 协议支持运行中状态传递。应如何保留该能力，同时避免任意修改 system prompt、持久化配置、数据库或审计策略？

**决策**:

- `call_function` / `call_workflow` 将 `action_params.args` 对象作为动作输入；两类动作都叠加可信 HookContext 顶层字段，最后注入 `_agent_context`。固定优先级 `args < runtime HookContext < _agent_context` 避免配置伪造 Agent/session/request/trigger 与上下文快照。
- Function/Workflow 动作输入包含当前 `AgentContext` 的序列化 `_agent_context` 快照，供动作读取 user input、messages、records 与 extensions。
- 只有动作输出顶层 `_agent_context_updates` 会触发状态更新。运行时固定映射三类操作：`records` → `set_record`、`extensions` → `add_extension`、字符串 `metadata` → `set_metadata`。
- Workflow 在每层节点完成后立即应用节点输出中的 updates，使下游节点可读取新快照；所有公共 end-output 合成路径都剥离内部 `_agent_context_updates` 字段，显式 output schema 声明该字段时以 5005/HTTP 422 和固定安全消息拒绝。
- 更新范围仅为当前 Agent 执行的内存 `AgentContext`。其他输出字段、Hook payload 和外部请求不能借此改写持久化 Agent 配置、system prompt 或 `RuntimeExecutionContext` 的 audit persistence mode。
- Function/Workflow 若执行数据库或外部系统业务副作用，仍须遵守被选动作的既有合同；经过 Plugin/Capability 的路径继续走 Agent permissions、Capability Registry 与 Dispatcher。受控 updates 不提供旁路，Hook 子链 audit 继续使用 sticky tracing-only 上下文。

**理由**:

- 受控输出协议让 Hook 的预处理、扩展卡片和循环控制结果能被后续步骤消费，而无需共享可变 payload 或引入 Hook 专用状态存储
- 固定入口和 `AgentContext` API 比解释任意 Function 输出更容易审查、测试和脱敏
- 内存范围明确区分了“当前运行状态更新”和“持久化 Agent 配置变更”，避免把 Hook CRUD、业务副作用与 runtime audit 混为一谈
- 保留现有 capability 权限检查，避免 Hook 因为运行在生命周期边界而获得额外外部写权限

**无效输入策略**: 缺少 update 字段时无操作；未知 record category、无法添加的 extension/record 与非字符串 metadata 值安全跳过。错误 tracing 只使用静态 `error_kind`，不包含原始 update payload。

**替代方案**:

- 保持严格只读，删除所有 `apply_agent_context_updates` — 被拒绝；会破坏已存在的运行时扩展、卡片和 loop-break 行为。
- 将任意 Function/Workflow 输出合并进 AgentContext — 被拒绝；状态边界不清晰且容易污染上下文。
- 把 updates 直接持久化到 Agent 配置或独立状态表 — 被拒绝；扩大 Hook 权限和存储范围，也违背当前仅需会话内状态传递的需求。
