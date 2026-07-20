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

### 无执行历史接口

`GET /api/agents/:id/hooks/executions` 以及全局 Hook 执行历史查询均不属于当前 API 合同。Hook 执行结果仅通过结构化 tracing 提供，不返回可分页的历史数据。

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
| 6003 | 400 | Webhook URL 不合法（仅支持 HTTPS 且不允许内网地址） |
| 6004 | 408 | Hook 执行超时 |
| 6005 | 500 | Hook（阻塞模式）执行失败，Agent 流程已中止 |
| 6006 | 404 | Hook 配置不存在 |
