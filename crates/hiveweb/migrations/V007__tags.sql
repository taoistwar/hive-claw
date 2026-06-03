CREATE TABLE tags (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    name VARCHAR(32) NOT NULL UNIQUE,
    color VARCHAR(7) NULL COMMENT 'hex 颜色，可选',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;

CREATE TABLE taggings (
    tag_id BIGINT NOT NULL,
    entity_type VARCHAR(16) NOT NULL COMMENT 'plugin|function|tool|skill|agent',
    entity_id BIGINT NOT NULL,
    PRIMARY KEY (tag_id, entity_type, entity_id),
    INDEX idx_taggings_entity (entity_type, entity_id),
    CONSTRAINT fk_taggings_tag FOREIGN KEY (tag_id) REFERENCES tags(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;
