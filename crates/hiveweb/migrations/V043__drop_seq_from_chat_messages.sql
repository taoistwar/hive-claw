-- Drop seq column from chat_messages_admin and chat_messages_user
-- seq is no longer needed after migration to per-session message ordering

ALTER TABLE chat_messages_admin DROP INDEX uk_chat_msg_admin_session_seq;
ALTER TABLE chat_messages_admin DROP COLUMN seq;

ALTER TABLE chat_messages_user DROP INDEX uk_chat_msg_user_session_seq;
ALTER TABLE chat_messages_user DROP COLUMN seq;
