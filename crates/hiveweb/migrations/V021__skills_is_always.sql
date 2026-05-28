-- Add `is_always` column to skills table
-- is_always=1 → skill is automatically loaded for all agents (no agent_skills entry needed)

ALTER TABLE skills
    ADD COLUMN is_always TINYINT(1) NOT NULL DEFAULT 0 COMMENT '0=normal, 1=always available for all agents'
    AFTER source;
