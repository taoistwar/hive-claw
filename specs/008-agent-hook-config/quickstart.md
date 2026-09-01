# Quickstart: Agent Hook 配置管理

**Feature**: `008-agent-hook-config`

---

## 前提条件

- 项目 `crates/hiveweb` 已启动并运行（`cargo run` 或 `restart-hiveweb.sh`）
- MySQL 数据库 migration 已执行
- 管理后台 `web-admin` 已启动（`npm run dev`）
- 已存在至少一个 Agent（非 main）用于测试

---

## 环境变量

在 `crates/hiveweb/.env` 中确认或新增：

```env
HOOK_TIMEOUT_MS=10000
HOOK_WEBHOOK_RETRY_MAX=3
```

`HOOK_WEBHOOK_RETRY_MAX` 仅允许 `0..=3`：`0` 禁用后台重试，缺失、非法或越界
值回退默认 `3`；三次退避固定为 1/2/4 秒。每次首次发送和 retry 的
`HOOK_TIMEOUT_MS` 预算都从 DNS 解析前开始，覆盖 client 构造与 send。
只有 typed `ResolveUnavailable`（DNS 暂时不可用）、reqwest connection error
（`is_connect()`）和 attempt timeout 进入后台重试。`Policy`、header、client
build、其他 Request 错误及 HTTP non-2xx 均不会重试。

---

## 验证步骤

### Step 1: 创建 Hook 配置（US1 核心流程）

1. 打开管理后台，进入 Agent 编辑页面
2. 找到 **Hook 配置** 区域（新增的 Tab 页或折叠面板）
3. 点击 **添加 Hook**，填写：
   - 名称: `测试钩子`
   - 触发点: `before_agent_start`
   - 动作类型: `call_function`
   - 选择 Function: `format_template`
   - 参数: `{"template": "Agent 开始执行: {{identifier}}"}`
4. 点击保存 → 验证列表中出现该 Hook

**预期**: Hook 保存成功，列表中展示名称、触发点、动作类型、启用状态。

### Step 2: 触发 Hook 执行（US1 验证）

1. 通过用户端调用同步 JSON 接口 `POST /api/assistant` 触发一次对话（不使用已移除的管理聊天或 admin SSE 端点）
2. 等待对话完成后，在服务日志中按 `event=hook_execution`、Agent ID 或 request ID 检索结构化事件
3. 如环境中仍有遗留 `hook_executions` 表，可在触发前后分别执行以下查询：

```sql
SELECT COUNT(*) FROM hook_executions;
```

**预期**: 日志中出现 `outcome=success`、`trigger_point=before_agent_start` 的结构化事件；触发前后的数据库行数相同。

### Step 3: 验证触发点上限（FR-001 边界）

1. 在同一触发点（如 `before_agent_start`）下配置 6 个 Hook
2. 保存第 6 个时

**预期**: 返回错误码 6001，提示"该触发点最多配置 5 个 Hook"。

### Step 4: 验证引用失效拒绝（FR-006）

1. 创建 Hook，动作类型选 `call_function`，选择一个不存在的 function_id
2. 点击保存

**预期**: 返回错误码 6002，提示"引用的 Function 不存在"。

### Step 5: Webhook 验证（US2）

1. 创建 Hook：
   - 触发点: `after_agent_end`
   - 动作类型: `http_webhook`
   - URL: `https://webhook.site/your-test-url`（可使用 webhook.site 生成测试端点）
2. 触发 Agent 对话
3. 检查 webhook.site 收到 POST 请求

**预期**: 外部端点收到包含 `agent_identifier`、`session_id`、`trigger_point`、`timestamp` 的 JSON payload。

### Step 6: Webhook URL 校验（FR-007）

1. 创建 Hook，URL 填写 `http://example.com`（非 HTTPS）
2. 分别尝试 `https://127.0.0.1`、`https://[::1]`、metadata hostname，以及在
   既有 Webhook 上仅 PUT 新的私网 `action_params`

**预期**: 均返回错误码 6003。公网测试端点执行时 3xx 不跟随；执行与每次
重试重新校验全部 DNS 答案并 pin 本次地址。

### Step 7: Workflow 调用验证（US3）

1. 若平台有已配置的 Workflow，创建 Hook：
   - 动作类型: `call_workflow`
   - 选择目标 Workflow
2. 触发 Agent 对话
3. 检查 `event=hook_execution` 的结构化日志

**预期**: 有 `outcome=success` 的结构化事件，数据库没有新增 Hook 执行记录。

### Step 8: 阻塞 Hook 错误语义

1. 配置一个 `blocking_mode=true` 的 `before_agent_start` Hook，并分别让其超时和返回执行失败
2. 调用 `POST /api/assistant`
3. 检查同步 JSON 响应、Redis 当日配额、assistant 消息以及 tracing

**预期**:

- 超时返回 HTTP 408 与 `{"code":6004,"message":"普通文本"}`。
- 其他阻塞失败返回 HTTP 500 与 `{"code":6005,"message":"普通文本"}`。
- 当日配额恰好回滚一次；Redis 回滚失败会产生 tracing，但不覆盖原始 Hook 错误。
- `on_agent_error` 以 tracing-only 方式执行；`after_agent_end` 不执行，也不保存 assistant 占位消息。
- 每个成功 hop 固定其 Hook 快照，下一 hop 重新 `fetch_content`；后续加载失败使用最近成功快照，首次失败无快照时跳过。其他终止错误（模型预设、LLM、路由循环、最大跳数等）同样触发 `on_agent_error`；整个阶段最多 30 秒，阶段失败或耗尽后响应仍保留原始错误。

### Step 9: 验证 Hook 配置缓存失效

1. 先执行一次 Agent hop，使 `agent:content:{id}` 缓存建立
2. 通过 Hook POST / PUT / DELETE 成功修改配置
3. 触发下一 hop 或下一次 Agent 执行

**预期**: 当前 hop 继续使用已固定快照；mutation 成功后缓存立即请求失效，下一次成功的 `fetch_content` 使用新 Hook 配置。若 Redis 删除失败，mutation 仍成功，并产生 `event=agent_content_cache_invalidation`、`error_kind=redis_delete_failed` 的静态 tracing。

### Step 10: 验证显式执行上下文与 Hook tracing-only 边界

先运行不依赖外部基础设施的聚焦测试：

```bash
cargo test -p hiveweb hook_execution_context_preserves_correlation_but_is_permanently_tracing_only
cargo test -p hiveweb record_enqueues_normal_context_but_never_hook_context
cargo test -p hiveweb runtime_chains_thread_one_explicit_context_without_task_local_state
```

随后可配置一个仅由 Hook 触发的 Workflow → Plugin → Capability 测试链，调用 `POST /api/assistant`，从响应 `X-Request-Id` header 和返回消息的 session 信息取得关联 ID，再检索 tracing 与 `runtime_audit_logs`。

**预期**:

- 普通 Assistant 上下文可向有界 best-effort audit 队列入队，记录包含本次 request/session 关联 ID。
- 进入 Hook 后关联 ID 保持不变；Hook execution、Workflow node、Plugin invoke 和 Capability call 的结构化 audit tracing 可按同一关联 ID 串联。
- Hook 子链不会向 runtime audit DB 队列入队；重复 `for_hook()` 或继续进入 Workflow/Plugin/Capability 也不会恢复 DB audit。
- 上下文通过显式依赖传递，不依赖 task-local；外部 payload 不能提供或覆盖 audit persistence mode。

### Step 11: 验证受控 `AgentContext` 更新

1. 准备一个只返回固定测试结果的 Function 或 Workflow，并让输出包含：

```json
{
  "_agent_context_updates": {
    "records": [
      {"category": "StateChanges", "key": "hook_ready", "value": true}
    ],
    "metadata": {
      "hook_marker": "ready"
    }
  },
  "ignored_top_level_state": "must-not-be-applied"
}
```

2. 将它配置为 `before_llm_call` Hook，调用 `POST /api/assistant`。
3. 在后续 Workflow 节点或测试 Hook 中读取 `_agent_context.state_changes` 与 metadata，并检查 tracing / runtime audit DB。

**预期**:

- `action_params.args` 的普通字段进入 Function/Workflow 调用输入；两类动作中
  伪造的 `agent_id` / `request_id` / `trigger_point` 都由可信 HookContext
  覆盖，伪造的 `args._agent_context` 再由运行时快照覆盖。
- `hook_ready=true` 与 `hook_marker=ready` 只出现在本次运行的 `AgentContext`，后续步骤可以读取。
- `ignored_top_level_state` 不会被解释为上下文更新；缺失/未知 update 项安全跳过。
- `_agent_context_updates` 不出现在 Workflow 的公共 end-output；虚拟 end 节点 output schema 若声明该键，Graph PUT 返回 5005/HTTP 422 与固定安全消息。
- 持久化 Agent 配置和 system prompt 不被直接改写；若 Function/Workflow 产生业务副作用，仍受被选动作的既有合同约束，Plugin/Capability 路径继续执行 Agent permission 与 Capability 鉴权。
- Hook execution 及其 Workflow/Plugin/Capability 子链仍只产生带原 request/session 关联 ID 的 tracing，不向 runtime audit DB 队列入队。

---

## 故障排查

| 症状 | 可能原因 | 检查 |
|------|---------|------|
| Hook 保存后列表为空 | V022 migration 未执行 | `SHOW TABLES LIKE 'agent_hooks'` |
| 下一 hop 暂未观察到新 Hook | AgentContent 缓存失效失败或 Redis 短暂异常 | 检查 `event=agent_content_cache_invalidation`；确认后续 Redis miss / DB fallback / TTL 到期后已收敛 |
| 对话后数据库没有执行记录 | 正常行为 | Hook 执行结果只输出 tracing，不写数据库 |
| 日志中没有 `hook_execution` | Hook disabled、日志级别过滤或 orchestrator 未集成 | 检查 Hook enabled 状态和 `RUST_LOG` 配置 |
| Webhook POST 未到达 | 防火墙/SSRF 拦截或 URL 不可达 | 检查 server 日志中的 SSRF rejection 记录 |
| 6001 在 5 个内触发 | sort_order 冲突 | 检查 `idx_agent_hooks_seq` unique 约束 |
