-- Split chat_sessions and chat_messages into separate admin/user tables
-- Admin tables: chat_sessions_admin + chat_messages_admin
-- User tables: chat_sessions_user + chat_messages_user

SET FOREIGN_KEY_CHECKS = 0;

-- 1. Create chat_sessions_admin (admin_id is required)
CREATE TABLE IF NOT EXISTS chat_sessions_admin (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    admin_id BIGINT NOT NULL COMMENT '发起测试的管理员',
    admin_phone_snapshot VARCHAR(11) NOT NULL DEFAULT '' COMMENT '快照：admin 删除后仍可追溯',
    admin_nickname_snapshot VARCHAR(20) NOT NULL DEFAULT '' COMMENT '快照：admin 删除后仍可追溯',
    title VARCHAR(128) NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    INDEX idx_chat_sessions_admin_id (admin_id),
    INDEX idx_chat_sessions_admin_updated_at (updated_at DESC),
    CONSTRAINT fk_chat_sessions_admin_admin FOREIGN KEY (admin_id) REFERENCES admins(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- 2. Create chat_sessions_user (user_id is required)
CREATE TABLE IF NOT EXISTS chat_sessions_user (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    user_id BIGINT NOT NULL COMMENT '关联的普通用户',
    user_phone_snapshot VARCHAR(11) NOT NULL DEFAULT '' COMMENT '快照：user 删除后仍可追溯',
    user_nickname_snapshot VARCHAR(64) NOT NULL DEFAULT '' COMMENT '快照：user 删除后仍可追溯',
    title VARCHAR(128) NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    INDEX idx_chat_sessions_user_id (user_id),
    INDEX idx_chat_sessions_user_updated_at (updated_at DESC),
    CONSTRAINT fk_chat_sessions_user_user FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- 3. Create chat_messages_admin (references chat_sessions_admin)
CREATE TABLE IF NOT EXISTS chat_messages_admin (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    session_id BIGINT NOT NULL,
    seq INT NOT NULL COMMENT '会话内单调序号',
    role VARCHAR(16) NOT NULL COMMENT 'user|assistant|tool|system',
    content TEXT NULL,
    tool_calls JSON NULL COMMENT '[{tool_call_id, name, args}]',
    routed_to_agent_id BIGINT NULL COMMENT '如果该消息触发了路由',
    elapsed_ms INT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE KEY uk_chat_msg_admin_session_seq (session_id, seq),
    INDEX idx_chat_messages_admin_session (session_id, created_at),
    CONSTRAINT fk_chat_msg_admin_session FOREIGN KEY (session_id) REFERENCES chat_sessions_admin(id) ON DELETE CASCADE,
    CONSTRAINT fk_chat_msg_admin_routed FOREIGN KEY (routed_to_agent_id) REFERENCES agents(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- 4. Create chat_messages_user (references chat_sessions_user)
CREATE TABLE IF NOT EXISTS chat_messages_user (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    session_id BIGINT NOT NULL,
    seq INT NOT NULL COMMENT '会话内单调序号',
    role VARCHAR(16) NOT NULL COMMENT 'user|assistant|tool|system',
    content TEXT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE KEY uk_chat_msg_user_session_seq (session_id, seq),
    INDEX idx_chat_messages_user_session (session_id, created_at),
    CONSTRAINT fk_chat_msg_user_session FOREIGN KEY (session_id) REFERENCES chat_sessions_user(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

-- 5. Migrate existing data from chat_sessions/chat_messages to new tables
-- Move admin sessions
INSERT INTO chat_sessions_admin (admin_id, admin_phone_snapshot, admin_nickname_snapshot, title, created_at, updated_at)
SELECT admin_id, admin_phone_snapshot, admin_nickname_snapshot, title, created_at, updated_at
FROM chat_sessions
WHERE admin_id IS NOT NULL;

-- Move user sessions
INSERT INTO chat_sessions_user (user_id, user_phone_snapshot, user_nickname_snapshot, title, created_at, updated_at)
SELECT user_id, user_phone_snapshot, user_nickname_snapshot, title, created_at, updated_at
FROM chat_sessions
WHERE user_id IS NOT NULL;

-- Migrate admin session messages
INSERT INTO chat_messages_admin (session_id, seq, role, content, tool_calls, routed_to_agent_id, elapsed_ms, created_at)
SELECT cm.session_id, cm.seq, cm.role, cm.content, cm.tool_calls, cm.routed_to_agent_id, cm.elapsed_ms, cm.created_at
FROM chat_messages cm
INNER JOIN chat_sessions cs ON cm.session_id = cs.id
WHERE cs.admin_id IS NOT NULL;

-- Migrate user session messages
INSERT INTO chat_messages_user (session_id, seq, role, content, created_at)
SELECT cm.session_id, cm.seq, cm.role, cm.content, cm.created_at
FROM chat_messages cm
INNER JOIN chat_sessions cs ON cm.session_id = cs.id
WHERE cs.user_id IS NOT NULL;

-- 6. Drop old tables after migration
DROP TABLE IF EXISTS chat_messages;
DROP TABLE IF EXISTS chat_sessions;

SET FOREIGN_KEY_CHECKS = 1;
