-- V050: hook_executions 表 — Hook 执行审计日志
-- 审计保留策略：无 CASCADE FK，Agent 删除后执行记录不丢失。
-- agent_identifier 快照列用于展示"已删除的 Agent {identifier}"。
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

    CONSTRAINT chk_hook_exec_outcome CHECK (
        outcome IN ('success', 'error', 'timeout', 'skipped')
    ),

    -- hook_id: SET NULL on agent_hooks row deletion (not CASCADE)
    -- agent_id: intentionally no FK — audit retention after Agent deletion

    INDEX idx_hook_exec_agent (agent_id, created_at DESC),
    INDEX idx_hook_exec_hook (hook_id),
    INDEX idx_hook_exec_session (session_id),
    INDEX idx_hook_exec_request (request_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
