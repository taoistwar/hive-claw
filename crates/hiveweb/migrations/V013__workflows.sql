CREATE TABLE workflows (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    identifier VARCHAR(64) NOT NULL UNIQUE,
    name VARCHAR(128) NOT NULL,
    description VARCHAR(512) NULL,
    timeout_ms INT NOT NULL DEFAULT 30000,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE workflow_nodes (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    workflow_id BIGINT NOT NULL,
    node_key VARCHAR(32) NOT NULL COMMENT '工作流内唯一 key',
    function_id BIGINT NOT NULL,
    position JSON NULL COMMENT 'reactflow 坐标 {x, y}',
    UNIQUE KEY uk_workflow_node (workflow_id, node_key),
    INDEX idx_workflow_nodes_workflow (workflow_id),
    CONSTRAINT fk_workflow_nodes_workflow FOREIGN KEY (workflow_id) REFERENCES workflows(id) ON DELETE CASCADE,
    CONSTRAINT fk_workflow_nodes_function FOREIGN KEY (function_id) REFERENCES functions(id) ON DELETE RESTRICT
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE workflow_edges (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    workflow_id BIGINT NOT NULL,
    src_node_id BIGINT NOT NULL,
    dst_node_id BIGINT NOT NULL,
    mapping JSON NOT NULL COMMENT '形如 {"dst.input.foo": "src.output.bar"}',
    INDEX idx_workflow_edges_workflow (workflow_id),
    INDEX idx_workflow_edges_src (src_node_id),
    INDEX idx_workflow_edges_dst (dst_node_id),
    CONSTRAINT fk_workflow_edges_workflow FOREIGN KEY (workflow_id) REFERENCES workflows(id) ON DELETE CASCADE,
    CONSTRAINT fk_workflow_edges_src FOREIGN KEY (src_node_id) REFERENCES workflow_nodes(id) ON DELETE CASCADE,
    CONSTRAINT fk_workflow_edges_dst FOREIGN KEY (dst_node_id) REFERENCES workflow_nodes(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
