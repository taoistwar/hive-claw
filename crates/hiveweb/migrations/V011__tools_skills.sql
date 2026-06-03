CREATE TABLE tools (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    identifier VARCHAR(64) NOT NULL UNIQUE,
    name VARCHAR(128) NOT NULL,
    description VARCHAR(512) NOT NULL,
    kind TINYINT NOT NULL COMMENT '1=function, 2=workflow',
    source VARCHAR(16) NOT NULL DEFAULT 'workspace' COMMENT 'workspace | builtin',
    is_always TINYINT(1) NOT NULL DEFAULT 0 COMMENT '0=normal, 1=always available for all agents',
    function_id BIGINT NULL,
    workflow_id BIGINT NULL,
    input_schema JSON NOT NULL,
    output_schema JSON NOT NULL,
    category_id BIGINT NULL,
    required_capabilities JSON NULL COMMENT '声明该 tool 执行所需的 capabilities',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    CONSTRAINT chk_tools_target CHECK (
        (kind = 1 AND workflow_id IS NULL) OR
        (kind = 2 AND function_id IS NULL)
    ),
    CONSTRAINT fk_tools_function FOREIGN KEY (function_id) REFERENCES functions(id) ON DELETE RESTRICT,
    CONSTRAINT fk_tools_workflow FOREIGN KEY (workflow_id) REFERENCES workflows(id) ON DELETE RESTRICT,
    CONSTRAINT fk_tools_category FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE skills (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    identifier VARCHAR(64) NOT NULL UNIQUE,
    name VARCHAR(128) NOT NULL,
    description VARCHAR(512) NOT NULL COMMENT 'frontmatter 中的简短描述',
    frontmatter JSON NULL COMMENT 'YAML frontmatter 解析后的结构',
    content MEDIUMTEXT NOT NULL COMMENT 'markdown 主体；建议 ≤ 64KB',
    source VARCHAR(16) NOT NULL DEFAULT 'workspace' COMMENT 'workspace | builtin',
    is_always TINYINT(1) NOT NULL DEFAULT 0 COMMENT '0=normal, 1=always available for all agents',
    category_id BIGINT NULL,
    required_capabilities JSON NULL COMMENT '声明该 skill 执行所需的 capabilities（来源于它所调用的 function 或 workflow）',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    CONSTRAINT fk_skills_category FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
