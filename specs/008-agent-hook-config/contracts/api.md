# API Contracts: Agent Hook 配置管理

**Feature**: `008-agent-hook-config`
**Status**: Draft
**Base Path**: `/api/agents/:id/hooks`

---

## Endpoints

### Hook CRUD

#### `POST /api/agents/:id/hooks`
创建 Hook 配置。

**Auth**: Admin JWT (System+ for non-main; Super for main)
**Request**: `CreateHookRequest` JSON
**Response**: `{ code: 0, data: AgentHook }`
**Errors**: 6001 (trigger limit exceeded), 6002 (invalid reference), 6003 (invalid webhook URL)

#### `GET /api/agents/:id/hooks`
列出 Agent 全部 Hook。

**Auth**: Admin JWT
**Response**: `{ code: 0, data: [AgentHook, ...] }` (sorted by trigger_point + sort_order)

#### `PUT /api/agents/:id/hooks/:hook_id`
更新 Hook 配置。

**Auth**: Admin JWT (same RBAC as POST)
**Request**: `UpdateHookRequest` JSON (must include `updated_at` for optimistic lock)
**Response**: `{ code: 0, data: AgentHook }`
**Errors**: 4094 (optimistic lock conflict), 6006 (not found)

#### `DELETE /api/agents/:id/hooks/:hook_id`
删除 Hook。

**Auth**: Admin JWT (same RBAC as POST)
**Response**: `{ code: 0, message: "success" }`
**Errors**: 6006 (not found)

上述 POST / PUT / DELETE 仅在数据库 mutation 成功后 best-effort 失效对应 `agent:content:{id}` 缓存。失效失败不覆盖成功响应，只输出固定 `redis_delete_failed` tracing；下一次 `fetch_content` 在缓存失效、Redis fallback 或 TTL 到期后读取新配置。

### 无执行历史接口

`GET /api/agents/:id/hooks/executions` 以及全局 Hook 执行历史查询均不属于当前 API 合同。Hook 执行结果仅通过结构化 tracing 提供，不返回可分页的历史数据。

### Runtime audit 关联与持久化边界（内部合同）

- HTTP 边界只接受 canonical non-nil UUID 形式的 `X-Request-Id`（固定 36 字节；统一为小写）。缺失或不合法时生成新 UUID；进入任何下游 middleware/handler 前，请求 `HeaderMap` 中的该 header 也必须被规范化值覆盖。handler 读取、响应 `X-Request-Id`、`RuntimeExecutionContext`、HookContext 与 tracing 使用同一值，原始不可信 header 不记录也不向下游暴露。
- `POST /api/assistant` 在服务端创建显式 `RuntimeExecutionContext`，携带本次规范化 `request_id` 和已解析的 `session_id`；客户端没有可用于选择 audit persistence mode 的请求字段。
- 普通 Assistant 调用树的 runtime audit 始终输出结构化 tracing，并可通过有界队列 best-effort 写入 `runtime_audit_logs`。
- Orchestrator 进入 Hook 时通过父上下文的 `for_hook()` 派生受限上下文：request/session 关联 ID 不变，audit persistence mode 单向降级为 tracing-only。
- Hook → Workflow → Plugin → Capability 必须继续显式传递该受限上下文。重复派生仍是 tracing-only，任一子层都不得通过默认值、反序列化或重新构造普通上下文恢复 DB audit。
- Hook 生命周期、Webhook retry 和嵌套 runtime audit tracing 保留父级 request/session 关联 ID；Hook 子链不会出现在 `runtime_audit_logs` 中。该规则不禁止配置查询或业务动作所需的正常数据库访问。

### Hook action `AgentContext` 合同（内部合同）

`call_function` / `call_workflow` 动作可读取运行时注入的 `_agent_context` 快照。该值是序列化输入，不是可变上下文句柄，也不得写入 Hook 执行历史或 runtime audit payload。

- 两类动作都将 `action_params.args` 对象合并到实际调用输入；省略或不是对象时按空对象处理。
- 运行时 `_agent_context` 必须最后注入并覆盖 `args` 中的同名字段，配置不得伪造上下文快照。
- `call_function` 与 `call_workflow` 都携带可信 HookContext 顶层字段；这些运行时字段覆盖 `args` 中的同名字段。
- 合并优先级固定为 `action_params.args < runtime HookContext < _agent_context`。可信字段包括 `agent_id`、`identifier`、`session_id`、`actor_id`、`request_id`、`trigger_point`、`message`、`channel`、`client_type`、`client_version`。

### HTTP Webhook 出站合同

- 创建 Hook，以及 PUT 中任何 `action_type` / `action_params` 变化，都对合并后的有效动作重新校验；仅修改 `action_params` 不能绕过。
- URL 必须可解析且仅为 HTTPS，不得携带 credentials；metadata hostname、IP literal 或 DNS 完整答案集中任一非公网 IPv4/IPv6 地址均返回 6003。
- 执行与每次后台重试重新解析全部地址并应用同一策略，请求只连接本次已校验并 pin 的地址；禁用环境代理和重定向。
- 自定义 header 通过实际 HTTP header parser；拒绝非字符串、CR/LF、非法 name/value 与 `Host` 覆盖。URL/地址策略拒绝返回 typed `Policy`，DNS 暂时不可用返回 `ResolveUnavailable`。
- Hook 只重试三类 transient failure：`ResolveUnavailable`、reqwest connection error（`is_connect()`）和 attempt timeout。`Policy`、header 解析、client build、其他 reqwest `Request` 错误及任意 HTTP non-2xx 均不可重试；分类不得检查错误字符串或按状态码范围放宽。
- 每次 attempt 由唯一 Tokio timeout 覆盖 DNS 解析、全答案校验、pinned client
  构造与 send；`run_hooks` 不为 Webhook 叠加相同 deadline。首次 timeout 在返回
  6004/timeout 语义前同步确定是否创建后台 retry，每次 retry 获得新的完整预算。
- `HOOK_WEBHOOK_RETRY_MAX` 仅接受 0..=3：默认 3、0 禁用、无效或越界值回退
  3；实际退避从固定 `[1, 2, 4]` 秒表取值，不使用可能溢出的指数运算。

动作若要影响本次 Agent 执行的后续步骤，只能返回顶层 `_agent_context_updates`：

```json
{
  "_agent_context_updates": {
    "records": [
      {
        "category": "StateChanges",
        "key": "prepared",
        "value": true,
        "source": "hook_function",
        "iteration": 0
      }
    ],
    "extensions": [
      {
        "id": "support",
        "content_type": "card",
        "reply": null,
        "data": {"title": "需要帮助？"}
      }
    ],
    "metadata": {
      "agent_loop_break": "false"
    }
  }
}
```

- `records.category` 支持当前 `AgentContext` categories：`Entities`、`Intentions`、`ToolResults`、`QueryResults`、`WorkflowResults`、`ReasoningResults`、`Extensions`、`StateChanges`、`SubagentResults`；未知值跳过。
- `extensions.content_type` 识别 `card`、`image`、`suggestion`、`link`、`button`、`table`、`chart`、`object_ref`、`usage`；当前运行时将未知类型安全降级为 `card`。
- `metadata` 只应用字符串值；非字符串值忽略。
- 缺少 `_agent_context_updates` 或其他顶层输出字段不会修改上下文。单项应用失败只输出静态 `error_kind`，不记录原始 payload。
- `_agent_context_updates` 是内部保留输出键：节点更新应用完成后，所有 Workflow 公共 end-output 合成路径及 HTTP `node_results` 每个节点值的顶层都必须剥离它。虚拟 end 节点的 `position.output_schema.properties` 若声明该键，Graph PUT 必须以既有 `5005 / HTTP 422` 和静态消息 `Workflow output_schema 不得声明内部保留字段「_agent_context_updates」` 拒绝。
- updates 仅作用于当前内存 `AgentContext`，不直接修改持久化 Agent 配置、system prompt、数据库或 audit persistence mode。业务副作用仍受被选 Function/Workflow 的既有合同约束；经过 Plugin/Capability 的路径继续执行 Agent permission 与 Capability 鉴权。整个 Hook 子链仍为 sticky tracing-only。

### Agent 执行错误（外部 Assistant API）

Hook 的运行时错误通过既有外部入口 `POST /api/assistant` 的同步 JSON 响应返回，不提供也不恢复管理聊天或 admin SSE 端点。

```json
{
  "code": 6004,
  "message": "Hook「slow-hook」阻塞模式执行超时"
}
```

- 阻塞 Hook 超时：6004 / HTTP 408。
- 阻塞 Hook 其他执行失败：6005 / HTTP 500。
- `message` 是普通文本，不得包含再次序列化的错误 JSON。
- 无效或无法识别的内部错误降级为 5000 / HTTP 500。
- 失败请求的当日配额恰好回滚一次；回滚命令失败只记录 tracing，不覆盖原始 Hook 错误。
- 阻塞失败路径执行 tracing-only `on_agent_error`，跳过 `after_agent_end`，且不持久化 assistant 占位消息。
- 每个 hop 成功执行 `fetch_content` 后固定当前 Agent Hook 快照；下一 hop 重新 fetch 并在成功后替换。模型预设解析、LLM、阻塞 Hook、路由循环、最大跳数和后续 Agent 内容加载等终止错误均进入统一的 tracing-only `on_agent_error` 阶段；后续加载失败使用最近成功快照，首次加载失败、快照尚不存在时明确跳过。
- `on_agent_error` 整个阶段共享固定 30 秒总预算。阶段自身失败或预算耗尽只记录不含下游错误文本的安全 tracing，不覆盖或再次触发原始错误；最终同步 JSON 保持原始错误码和普通文本消息。

## Data Schemas

### AgentHook

```json
{
  "id": "bigint",
  "agent_id": "bigint",
  "name": "string",
  "description": "string | null",
  "trigger_point": "before_agent_start | after_agent_end | on_agent_error | before_tool_call | after_tool_call | before_llm_call | after_llm_call",
  "action_type": "call_function | call_workflow | http_webhook",
  "action_params": "object (varies by action_type)",
  "enabled": "boolean",
  "sort_order": "int",
  "blocking_mode": "boolean",
  "timeout_ms": "int",
  "created_at": "datetime",
  "updated_at": "datetime"
}
```

## Error Codes

| Code | HTTP | Message |
|------|------|---------|
| 6001 | 422 | 该触发点最多配置 5 个 Hook |
| 6002 | 422 | Hook 引用的 Function/Workflow 不存在或已删除 |
| 6003 | 400 | Webhook URL/header 不合法（仅支持 HTTPS，全部 DNS 答案必须为公网地址，且不得覆盖 Host） |
| 6004 | 408 | 阻塞模式 Hook 执行超时，Agent 流程已中止 |
| 6005 | 500 | Hook（阻塞模式）执行失败，Agent 流程已中止 |
| 6006 | 404 | Hook 配置不存在 |
