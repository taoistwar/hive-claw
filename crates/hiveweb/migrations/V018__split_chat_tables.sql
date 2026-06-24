-- Chat tables for end users (admin chat not implemented, dropped)

CREATE TABLE IF NOT EXISTS chat_sessions_user (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    user_id BIGINT NOT NULL COMMENT '关联的普通用户',
    user_phone_snapshot VARCHAR(100) NOT NULL DEFAULT '' COMMENT '快照：user 删除后仍可追溯',
    user_nickname_snapshot VARCHAR(64) NOT NULL DEFAULT '' COMMENT '快照：user 删除后仍可追溯',
    title VARCHAR(128) NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    INDEX idx_chat_sessions_user_id (user_id),
    INDEX idx_chat_sessions_user_updated_at (updated_at DESC),
    CONSTRAINT fk_chat_sessions_user_user FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE IF NOT EXISTS chat_messages_user (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    session_id BIGINT NOT NULL,
    user_id BIGINT NOT NULL COMMENT '关联的普通用户',
    role VARCHAR(16) NOT NULL COMMENT 'user|assistant|tool|system',
    content TEXT NULL,
    elapsed_ms INT NULL COMMENT 'assistant 消息耗时(毫秒)',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    INDEX idx_chat_messages_user_session (session_id, created_at),
    INDEX idx_chat_messages_user_user_id (user_id),
    CONSTRAINT fk_chat_msg_user_session FOREIGN KEY (session_id) REFERENCES chat_sessions_user(id) ON DELETE CASCADE,
    CONSTRAINT fk_chat_msg_user_user FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
