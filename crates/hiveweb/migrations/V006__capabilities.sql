CREATE TABLE capabilities (
    name VARCHAR(64) PRIMARY KEY COMMENT 'e.g. network.http',
    description VARCHAR(255) NOT NULL,
    is_dangerous TINYINT(1) NOT NULL DEFAULT 0 COMMENT '需 Super 才能授予',
    category_id BIGINT NULL COMMENT '所属分类',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT fk_capabilities_category FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE SET NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Capability 元数据；启动期由 runtime registry upsert';
