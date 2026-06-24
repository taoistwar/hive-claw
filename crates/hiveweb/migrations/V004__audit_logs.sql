-- T093 / spec FR-022：管理员操作审计日志，保留 ≥ 90 天。
-- operator_id 为操作发起者（claims.admin_id），可空（系统脚本写入时为空）。
-- target_admin_id 为被操作的管理员；删除后置 NULL，靠 snapshot 列保留可读性。

CREATE TABLE IF NOT EXISTS admin_audit_logs (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    operator_id BIGINT NULL,
    operator_phone_snapshot VARCHAR(11) NOT NULL DEFAULT '',
    target_admin_id BIGINT NULL,
    target_phone_snapshot VARCHAR(11) NOT NULL DEFAULT '',
    operation VARCHAR(32) NOT NULL COMMENT 'create | update | delete | enable | disable',
    detail JSON NULL COMMENT '可选：变更字段、变更前后值等',
    occurred_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    INDEX idx_target (target_admin_id),
    INDEX idx_operator (operator_id),
    INDEX idx_occurred_at (occurred_at DESC),
    CONSTRAINT fk_admin_audit_operator FOREIGN KEY (operator_id) REFERENCES admins(id) ON DELETE SET NULL,
    CONSTRAINT fk_admin_audit_target FOREIGN KEY (target_admin_id) REFERENCES admins(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='管理员操作审计（保留 90+ 天）';
