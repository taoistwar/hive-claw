-- Add user_id to chat_sessions so sessions can be associated with users
-- Keep admin_id for backward compatibility (legacy sessions)

ALTER TABLE chat_sessions
    ADD COLUMN user_id BIGINT NULL COMMENT '关联的普通用户；与 admin_id 二选一',
    ADD COLUMN user_phone_snapshot VARCHAR(11) NOT NULL DEFAULT '' COMMENT '快照：user 删除后仍可追溯',
    ADD COLUMN user_nickname_snapshot VARCHAR(64) NOT NULL DEFAULT '' COMMENT '快照：user 删除后仍可追溯',
    ADD INDEX idx_chat_sessions_user (user_id),
    ADD CONSTRAINT fk_chat_sessions_user FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE SET NULL;
