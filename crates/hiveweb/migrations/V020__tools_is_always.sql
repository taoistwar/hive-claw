-- Add `is_always` column to tools table
-- is_always=1 → tool is automatically loaded for all agents (no agent_tools entry needed)
-- Also relax CHECK constraint to allow kind=1 with function_id=NULL for meta-tools
-- (invoke_function / invoke_workflow are meta-tools that take identifier at runtime)

ALTER TABLE tools
    ADD COLUMN is_always TINYINT(1) NOT NULL DEFAULT 0 COMMENT '0=normal, 1=always available for all agents'
    AFTER source;

-- Drop old CHECK constraint and add relaxed version
ALTER TABLE tools DROP CONSTRAINT chk_tools_target;
ALTER TABLE tools
    ADD CONSTRAINT chk_tools_target CHECK (
        (kind = 1 AND workflow_id IS NULL) OR
        (kind = 2 AND function_id IS NULL)
    );
