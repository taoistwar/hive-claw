CREATE TABLE functions (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    identifier VARCHAR(64) NOT NULL UNIQUE COMMENT '全局唯一标识符',
    name VARCHAR(128) NOT NULL,
    description VARCHAR(512) NULL,
    kind TINYINT NOT NULL COMMENT '1=builtin, 2=custom',
    input_schema JSON NOT NULL COMMENT 'JSON Schema',
    output_schema JSON NOT NULL,
    plugin_id BIGINT NULL COMMENT 'custom 必填',
    plugin_export VARCHAR(64) NULL COMMENT 'extism export 函数名，custom 必填',
    category_id BIGINT NULL,
    required_capabilities JSON NULL COMMENT '声明该 function 执行所需的 capabilities',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    INDEX idx_functions_plugin (plugin_id),
    INDEX idx_functions_category (category_id),
    FULLTEXT INDEX ftx_functions (name, description, identifier),
    CONSTRAINT fk_functions_plugin FOREIGN KEY (plugin_id) REFERENCES plugins(id) ON DELETE RESTRICT,
    CONSTRAINT fk_functions_category FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE SET NULL,
    CONSTRAINT chk_functions_custom CHECK (
        (kind = 1) OR (kind = 2 AND plugin_id IS NOT NULL AND plugin_export IS NOT NULL)
    )
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
