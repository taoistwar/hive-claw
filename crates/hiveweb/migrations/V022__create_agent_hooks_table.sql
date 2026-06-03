-- V049: agent_hooks 表 — Agent Hook 配置
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

    CONSTRAINT chk_agent_hooks_trigger_point CHECK (
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

    CONSTRAINT chk_agent_hooks_action_type CHECK (
        action_type IN ('call_function', 'call_workflow', 'http_webhook')
    ),

    CONSTRAINT chk_agent_hooks_timeout_ms CHECK (timeout_ms >= 1000),

    CONSTRAINT fk_agent_hooks_agent FOREIGN KEY (agent_id)
        REFERENCES agents(id) ON DELETE CASCADE,

    INDEX idx_agent_hooks_agent (agent_id),
    UNIQUE INDEX idx_agent_hooks_seq (agent_id, trigger_point, sort_order)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
