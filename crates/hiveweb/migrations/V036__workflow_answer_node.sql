-- Add generate_answer_node type to workflow_nodes node_type ENUM
-- Remove NOT NULL constraint on function_id so answer nodes can omit it
-- Add node_config JSON column for answer node configuration (system_prompt, model_preset, variables, etc.)

ALTER TABLE workflow_nodes
    MODIFY COLUMN node_type ENUM('function_node','start_node','end_node','generate_answer_node') NOT NULL DEFAULT 'function_node';

ALTER TABLE workflow_nodes
    MODIFY COLUMN function_id BIGINT NULL;

ALTER TABLE workflow_nodes
    ADD COLUMN node_config JSON NULL COMMENT 'Node-specific configuration (e.g., answer node: system_prompt, model_preset, history_window, variables)' AFTER position;
