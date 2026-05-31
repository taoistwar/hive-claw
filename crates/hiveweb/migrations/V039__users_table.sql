-- Users table for end users (not admins)
-- Used by chat sessions and messages for user-level isolation

CREATE TABLE users (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    phone VARCHAR(11) NOT NULL UNIQUE COMMENT '手机号',
    password_hash VARCHAR(128) NOT NULL COMMENT '密码哈希 (bcrypt)',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP COMMENT '修改日志时间',
    last_login_at DATETIME DEFAULT NULL,
    INDEX idx_users_phone (phone),
    INDEX idx_users_created_at (created_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='用户表';
