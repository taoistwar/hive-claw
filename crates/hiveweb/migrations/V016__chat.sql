CREATE TABLE chat_sessions (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    admin_id BIGINT NULL COMMENT '发起测试的管理员；admin 删除后 SET NULL，session 仅 Super 可访问',
    admin_phone_snapshot VARCHAR(11) NOT NULL DEFAULT '' COMMENT '快照：admin 删除后仍可追溯',
    admin_nickname_snapshot VARCHAR(20) NOT NULL DEFAULT '' COMMENT '快照：admin 删除后仍可追溯',
    title VARCHAR(128) NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    INDEX idx_chat_sessions_admin (admin_id),
    INDEX idx_chat_sessions_updated_at (updated_at DESC),
    CONSTRAINT fk_chat_sessions_admin FOREIGN KEY (admin_id) REFERENCES admins(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE chat_messages (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    session_id BIGINT NOT NULL,
    seq INT NOT NULL COMMENT '会话内单调序号',
    role VARCHAR(16) NOT NULL COMMENT 'user|assistant|tool|system',
    content TEXT NULL,
    tool_calls JSON NULL COMMENT '[{tool_call_id, name, args}]',
    routed_to_agent_id BIGINT NULL COMMENT '如果该消息触发了路由',
    elapsed_ms INT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE KEY uk_chat_msg_session_seq (session_id, seq),
    INDEX idx_chat_messages_session (session_id, created_at),
    CONSTRAINT fk_chat_msg_session FOREIGN KEY (session_id) REFERENCES chat_sessions(id) ON DELETE CASCADE,
    CONSTRAINT fk_chat_msg_routed FOREIGN KEY (routed_to_agent_id) REFERENCES agents(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
