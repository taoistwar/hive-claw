# Research: Agent Hook 配置管理

**Feature**: `008-agent-hook-config`
**Status**: Draft
**Phase**: 0 — Technical Decisions

---

## 1. Hook 配置与 Agent Context 的加载策略

**问题**: 每次 `build_agent_context` 时如何加载 Agent 的 Hook 配置？JOIN 查询还是独立查询缓存？

**决策**: 在 `build_agent_context()` 中通过 `agent_hook::load_hooks_for_agent(pool, agent_id)` 一次性 JOIN 加载所有 enabled Hook，按 `trigger_point` 分组后存入 `AgentContext.hooks: HashMap<String, Vec<AgentHook>>`。

**理由**:
- Hook 配置量小（每 Agent 最多 35 个），单次 JOIN 开销可忽略
- 避免缓存失效复杂度（Hook 配置变更频繁度低，但即时生效要求 SC-002 不允许缓存延迟）
- 与会话快照语义一致（会话使用开始时的 Hook 快照，不受后续变更影响）
- 无需引入 Redis 缓存层（Principle V: YAGNI）

**替代方案**: Redis 缓存 + TTL 刷新 — 被拒绝，因增加基础设施依赖且 Hook 数据量不足以支撑缓存必要性。

---

## 2. Webhook 重试的持久化

**问题**: Webhook 后台重试期间的状态是否需要持久化到 DB？服务重启后重试是否会丢失？

**决策**: 不持久化重试状态或最终结果。重试通过 `tokio::spawn` 在内存中异步执行，重启后丢失未完成的重试；每次尝试和最终结果仅输出结构化 tracing。

**理由**:
- Webhook 重试窗口短（最长 7s: 1s+2s+4s），重启窗口内丢失概率低
- 引入重试状态表增加实现复杂度（Principle V: YAGNI）
- Webhook 本质是 best-effort 通知，不要求强可靠性
- 若外部端点不可达超过 7s，大概率需人工介入而非自动恢复
- 避免高频 Hook 执行结果持续扩大数据库容量

**替代方案**: 在数据库中保存执行结果或重试状态并由 cron 恢复 — 被拒绝，因数据量增长、实现复杂度与可靠性收益不匹配。

---

## 3. Hook 执行的并发模型

**问题**: orchestrator 中同一触发点的多个 Hook 应串行还是并行执行？

**决策**: 严格串行，按 `sort_order` 升序依次执行。

**理由**:
- 避免并发带来的状态管理复杂性（共享 `AgentContext`、`pool` 引用）
- Hook 为只读观察者，执行顺序不影响 Agent 状态，但影响结构化日志的可追溯性
- 串行执行天然支持阻塞模式（失败即中止后续 Hook）
- 每个 Hook 执行时间短（默认超时 10s），串行带来的总延迟可控
- 与 spec FR-012 一致

**替代方案**: 同一触发点内 Hook 并行执行 `tokio::join!` — 被拒绝，因阻塞模式下需要协调中止逻辑，且并发 tracing 事件顺序不可预测。

---

## 4. Hook 超时与 Agent 超时的交互

**问题**: Hook 执行超时是否计入 Agent 整体超时预算？

**决策**: 不计入。Hook 超时独立计时（`tokio::time::timeout`），Agent 整体超时仅计算 LLM 调用 + tool 执行时间。

**理由**:
- Hook 为可选的附加功能，不应惩罚未配置 Hook 的 Agent
- Hook 超时后强制中止（`tokio::time::timeout` 的 `Elapsed` 错误），不阻塞 Agent 主流程
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
