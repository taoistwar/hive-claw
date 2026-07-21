# Data Model: Agent Hook 配置管理

**Feature**: `008-agent-hook-config`
**Status**: Draft

---

## Entity Relationship

```
┌──────────┐       ┌───────────────┐
│  agents  │──1:N──│  agent_hooks  │
│          │       │               │
│   id PK  │       │  id PK        │
│          │       │  agent_id FK  │
└──────────┘       │  name         │
                   │  trigger_point│
                   │  action_type  │
                   │  action_params│
                   │  enabled      │
                   │  sort_order   │
                   │  blocking_mode│
                   │  timeout_ms   │
                   └───────────────┘
```

**关键设计**:
- `agent_hooks.agent_id → agents(id) ON DELETE CASCADE`: Agent 删除时 Hook 配置级联删除
- Hook 执行结果和重试结果不属于持久化实体，仅输出结构化 tracing
- 已部署环境中的遗留执行历史表和数据保持不动，但应用不再读写

---

## DDL

### V022: `agent_hooks` 表

```sql
CREATE TABLE agent_hooks (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    agent_id BIGINT NOT NULL,
    name VARCHAR(128) NOT NULL,
    description TEXT,
    trigger_point VARCHAR(32) NOT NULL,
    action_type VARCHAR(32) NOT NULL,
    action_params JSON NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    sort_order INT NOT NULL DEFAULT 0,
    blocking_mode BOOLEAN NOT NULL DEFAULT FALSE,
    timeout_ms INT NOT NULL DEFAULT 10000,
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
    updated_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),

    -- Constraints
    CONSTRAINT chk_agent_hooks_trigger CHECK (
        trigger_point IN (
            'before_agent_start',
            'after_agent_end',
            'on_agent_error',
            'before_tool_call',
            'after_tool_call',
            'before_llm_call',
            'after_llm_call'
        )
    ),
    CONSTRAINT chk_agent_hooks_action CHECK (
        action_type IN ('call_function', 'call_workflow', 'http_webhook')
    ),

    -- Foreign Keys
    CONSTRAINT fk_agent_hooks_agent FOREIGN KEY (agent_id)
        REFERENCES agents(id) ON DELETE CASCADE,

    -- Indexes
    INDEX idx_agent_hooks_agent (agent_id),
    UNIQUE INDEX idx_agent_hooks_seq (agent_id, trigger_point, sort_order)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
```

### V023: `hook_executions` 遗留表

该迁移已经发布，按 forward-only 迁移约定保留原文，以免破坏既有环境的迁移历史。自 2026-07-16 起，运行时、API 和管理界面均不再读写该表；本次变更也不删除其中已有数据。

```sql
CREATE TABLE hook_executions (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    agent_id BIGINT NOT NULL,
    agent_identifier VARCHAR(64) NOT NULL,
    hook_id BIGINT,
    session_id BIGINT,
    trigger_point VARCHAR(32) NOT NULL,
    action_type VARCHAR(32) NOT NULL,
    outcome VARCHAR(16) NOT NULL,
    error_summary TEXT,
    elapsed_ms INT,
    context_snapshot JSON,
    request_id VARCHAR(64),
    created_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    -- Constraints
    CONSTRAINT chk_hook_exec_outcome CHECK (
        outcome IN ('success', 'error', 'timeout', 'skipped')
    ),

    -- Foreign Keys
    CONSTRAINT fk_hook_exec_hook FOREIGN KEY (hook_id)
        REFERENCES agent_hooks(id) ON DELETE SET NULL,

    -- Indexes
    INDEX idx_hook_exec_agent (agent_id, created_at DESC),
    INDEX idx_hook_exec_hook (hook_id),
    INDEX idx_hook_exec_session (session_id),
    INDEX idx_hook_exec_request (request_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
```

---

## `action_params` JSON Schema

### `call_function`

```json
{
    "function_id": "bigint (required)",
    "args": {}
}
```

### `call_workflow`

```json
{
    "workflow_id": "bigint (required)",
    "args": {}
}
```

### `http_webhook`

```json
{
    "webhook_url": "string (required, https:// only)",
    "headers": {"string": "string"},
    "timeout_ms": "int (optional, default 10000)"
}
```

---

## 索引策略

| 表 | 索引 | 用途 |
|---|------|------|
| `agent_hooks` | `idx_agent_hooks_agent (agent_id)` | `load_hooks_for_agent` 按 agent_id 加载 |
| `agent_hooks` | `UNIQUE idx_agent_hooks_seq (agent_id, trigger_point, sort_order)` | 防止同触发点同序号重复 + 排序查询 |

---

## 执行可观测性策略

- 每次 Hook 执行输出结构化 tracing，包含关联 ID、动作类型、结果和耗时
- 失败事件仅输出白名单 `error_kind`，不输出任意下游错误文本、完整用户消息、Webhook URL、payload 或响应体
- Hook 执行不会新增数据库记录，因此不存在应用侧执行历史保留或清理任务
- 遗留表中的已有数据不在本次变更范围内，由运维另行决定归档或删除

---

## Scale Estimation

| 指标 | 值 |
|------|---|
| 最大 Agent 数 | 500 |
| 每 Agent 最大 Hook 数 | 35 (7 触发点 × 5) |
| `agent_hooks` 最大行数 | 17,500 |
| 每会话 Hook 执行次数 | ~10-20 (取决 LLM hop 数) |
| Hook 执行历史月增量 | 0 行 |
| 遗留执行历史数据 | 保持现状，不由应用继续增长 |
