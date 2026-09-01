-- 004 Agent Runtime: sanitized runtime audit persistence.
--
-- Runtime operations always emit structured tracing first. This table stores a
-- second, best-effort copy for Super administrators to query. Hook execution is
-- intentionally excluded and remains tracing-only.
--
-- No foreign keys are used: audit history must survive deletion of referenced
-- runtime resources. Retention is configurable and defaults to 36500 days
-- (100 years).
CREATE TABLE IF NOT EXISTS runtime_audit_logs (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    request_id VARCHAR(64),
    session_id BIGINT,
    agent_id BIGINT,
    plugin_id BIGINT,
    function_id BIGINT,
    capability VARCHAR(64),
    event_type VARCHAR(32) NOT NULL,
    outcome VARCHAR(16) NOT NULL,
    elapsed_ms INT,
    error_message VARCHAR(512),
    payload_summary JSON,
    occurred_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    INDEX idx_ral_request (request_id),
    INDEX idx_ral_session (session_id),
    INDEX idx_ral_agent (agent_id),
    INDEX idx_ral_occurred_at (occurred_at DESC),
    INDEX idx_ral_capability (capability, outcome)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci
  COMMENT='Sanitized non-Hook runtime audit records';
