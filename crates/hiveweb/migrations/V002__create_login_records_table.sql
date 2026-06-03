-- 登录记录表（data-model.md §LoginRecord）
-- admin 删除后 admin_id 置 NULL，保留审计快照以满足 90 天保留要求。
CREATE TABLE IF NOT EXISTS login_records (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    admin_id BIGINT NULL,
    admin_phone_snapshot VARCHAR(11) NOT NULL DEFAULT '',
    admin_nickname_snapshot VARCHAR(20) NOT NULL DEFAULT '',
    login_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    ip_address VARCHAR(45) NOT NULL COMMENT 'IPv4 or IPv6',
    success TINYINT(1) NOT NULL COMMENT '1=true, 0=false',
    failure_reason VARCHAR(50) DEFAULT NULL COMMENT 'WRONG_PASSWORD | ACCOUNT_DISABLED | ACCOUNT_LOCKED | OTHER',
    INDEX idx_admin_id (admin_id),
    INDEX idx_login_at (login_at DESC),
    CONSTRAINT fk_login_admin FOREIGN KEY (admin_id) REFERENCES admins(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='登录记录表（审计用，保留 90+ 天，admin 删除后 admin_id 置 NULL）';
