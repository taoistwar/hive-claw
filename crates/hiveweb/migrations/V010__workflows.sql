CREATE TABLE workflows (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    identifier VARCHAR(64) NOT NULL UNIQUE,
    name VARCHAR(128) NOT NULL,
    description VARCHAR(512) NULL,
    timeout_ms INT NOT NULL DEFAULT 180000,
    required_capabilities JSON NULL COMMENT '计算出的执行所需 capabilities（由 DAG 中所有节点的 function 聚合）',
    category_id BIGINT NULL COMMENT '所属分类',
    input_schema JSON NULL COMMENT '工作流起始节点的输入变量定义 (JSON Schema format)',
    start_description VARCHAR(512) NULL COMMENT '起始节点描述/欢迎语',
    output_schema JSON NULL COMMENT '工作流结束节点的输出变量定义 (JSON Schema format)',
    end_description VARCHAR(512) NULL COMMENT '结束节点描述/结束语',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    CONSTRAINT fk_workflows_category FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE workflow_nodes (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    workflow_id BIGINT NOT NULL,
    node_key VARCHAR(32) NOT NULL COMMENT '工作流内唯一 key',
    node_type ENUM('function_node','start_node','end_node','generate_answer_node') NOT NULL DEFAULT 'function_node' COMMENT '节点类型',
    function_id BIGINT NULL,
    position JSON NULL COMMENT 'reactflow 坐标 {x, y}',
    node_config JSON NULL COMMENT 'Node-specific configuration (e.g., answer node: system_prompt, model_preset, history_window, variables)',
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
