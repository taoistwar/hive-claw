-- 登录记录表（data-model.md §LoginRecord）
-- 初始 schema：admin_id NOT NULL + ON DELETE CASCADE；V004 会把它改为 SET NULL
-- 并补审计快照列。
CREATE TABLE IF NOT EXISTS login_records (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    admin_id BIGINT NOT NULL,
    login_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    ip_address VARCHAR(45) NOT NULL COMMENT 'IPv4 or IPv6',
    success TINYINT(1) NOT NULL COMMENT '1=true, 0=false',
    failure_reason VARCHAR(50) DEFAULT NULL COMMENT 'WRONG_PASSWORD | ACCOUNT_DISABLED | ACCOUNT_LOCKED | OTHER',
    INDEX idx_admin_id (admin_id),
    INDEX idx_login_at (login_at),
    CONSTRAINT fk_login_admin FOREIGN KEY (admin_id) REFERENCES admins(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='登录记录表';
