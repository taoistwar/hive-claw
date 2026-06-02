-- Add user_id to chat_messages_user and admin_id to chat_messages_admin
-- These columns are expected by the Rust models (ChatMessageUser/ChatMessageAdmin)
-- but were missed in the original V042 table definitions.

-- 1. Add user_id to chat_messages_user
ALTER TABLE chat_messages_user
    ADD COLUMN user_id BIGINT NOT NULL DEFAULT 0 COMMENT '关联的普通用户';

-- Backfill user_id from chat_sessions_user for existing records
UPDATE chat_messages_user cmu
INNER JOIN chat_sessions_user csu ON cmu.session_id = csu.id
SET cmu.user_id = csu.user_id
WHERE cmu.user_id = 0;

ALTER TABLE chat_messages_user
    ADD INDEX idx_chat_messages_user_user_id (user_id),
    ADD CONSTRAINT fk_chat_msg_user_user FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;

-- 2. Add admin_id to chat_messages_admin (same issue)
ALTER TABLE chat_messages_admin
    ADD COLUMN admin_id BIGINT NOT NULL DEFAULT 0 COMMENT '发起测试的管理员';

-- Backfill admin_id from chat_sessions_admin for existing records
UPDATE chat_messages_admin cma
INNER JOIN chat_sessions_admin csa ON cma.session_id = csa.id
SET cma.admin_id = csa.admin_id
WHERE cma.admin_id = 0;

ALTER TABLE chat_messages_admin
    ADD INDEX idx_chat_messages_admin_admin_id (admin_id),
    ADD CONSTRAINT fk_chat_msg_admin_admin FOREIGN KEY (admin_id) REFERENCES admins(id) ON DELETE CASCADE;
