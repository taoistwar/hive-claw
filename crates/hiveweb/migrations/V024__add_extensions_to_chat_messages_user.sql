-- Add extensions column to chat_messages_user for persisting AgentContext extension data (cards, images, etc.)
ALTER TABLE chat_messages_user
    ADD COLUMN extensions JSON NULL COMMENT 'AgentContext 扩展数据 (cards/images/suggestions 等)'
    AFTER elapsed_ms;
