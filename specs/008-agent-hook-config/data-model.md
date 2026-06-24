# Data Model: Agent Hook 配置管理

**Feature**: `008-agent-hook-config`
**Status**: Draft

---

## Entity Relationship

```
┌──────────┐       ┌───────────────┐       ┌──────────────────┐
│  agents  │──1:N──│  agent_hooks  │──1:N──│ hook_executions  │
│          │       │               │       │  (via hook_id    │
│   id PK  │       │  id PK        │       │   SET NULL)      │
│          │       │  agent_id FK  │       │                  │
└──────────┘       │  name         │       │  id PK           │
                   │  trigger_point│       │  agent_id (snap) │
                   │  action_type  │       │  hook_id FK?     │
                   │  action_params│       │  session_id      │
                   │  enabled      │       │  outcome         │
                   │  sort_order   │       │  ...             │
                   │  blocking_mode│       └──────────────────┘
                   │  timeout_ms   │
                   └───────────────┘
```

**关键设计**:
- `agent_hooks.agent_id → agents(id) ON DELETE CASCADE`: Agent 删除时 Hook 配置级联删除
- `hook_executions.hook_id → agent_hooks(id) ON DELETE SET NULL`: Hook 删除后执行历史保留，hook_id 置 NULL
- `hook_executions.agent_id` 无 FK: 审计保留快照，Agent 删除后执行记录不丢失
- `hook_executions.agent_identifier` 快照列: 供追溯"已删除的 Agent {identifier}"场景

---

## DDL

### V049: `agent_hooks` 表

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

### V050: `hook_executions` 表

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
| `hook_executions` | `idx_hook_exec_agent (agent_id, created_at DESC)` | 历史查询主索引（按 Agent + 时间） |
| `hook_executions` | `idx_hook_exec_hook (hook_id)` | 按 Hook 查询执行记录 |
| `hook_executions` | `idx_hook_exec_session (session_id)` | 按会话关联查询 |
| `hook_executions` | `idx_hook_exec_request (request_id)` | 按请求 ID 追踪全链路 |

---

## 数据保留策略

- `hook_executions` 数据默认保留 **30 天**（可配 `HOOK_EXECUTION_RETENTION_DAYS` env）
- 每日凌晨 cron 执行 `DELETE FROM hook_executions WHERE created_at < NOW() - INTERVAL N DAY`
- 清理逻辑以 batch 执行（每次 ≤ 1000 行），避免长事务锁表

---

## Scale Estimation

| 指标 | 值 |
|------|---|
| 最大 Agent 数 | 500 |
| 每 Agent 最大 Hook 数 | 35 (7 触发点 × 5) |
| `agent_hooks` 最大行数 | 17,500 |
| 每会话 Hook 执行次数 | ~10-20 (取决 LLM hop 数) |
| `hook_executions` 月增量 | ~500 Agent × 100 会话/天 × 10 执行 × 30 天 ≈ 15M |
| 30 天保留后稳态 | ~15M 行 |
