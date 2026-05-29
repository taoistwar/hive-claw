ALTER TABLE workflow_nodes ADD COLUMN node_type ENUM('function_node','start_node') NOT NULL DEFAULT 'function_node' COMMENT '节点类型' AFTER node_key;
