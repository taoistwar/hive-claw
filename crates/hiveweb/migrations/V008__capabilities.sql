CREATE TABLE capabilities (
    name VARCHAR(64) PRIMARY KEY COMMENT 'e.g. network.http',
    description VARCHAR(255) NOT NULL,
    is_dangerous TINYINT(1) NOT NULL DEFAULT 0 COMMENT '需 Super 才能授予',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='Capability 元数据；启动期由 runtime registry upsert';
