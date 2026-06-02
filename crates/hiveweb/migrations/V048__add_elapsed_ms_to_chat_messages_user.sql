-- Add elapsed_ms column to chat_messages_user (already exists in chat_messages_admin since V042)
-- The Rust model ChatMessageUser includes this field and append_assistant_message_user writes it.

ALTER TABLE chat_messages_user
    ADD COLUMN elapsed_ms INT NULL COMMENT 'assistant 消息耗时(毫秒)';
