-- T002: Create game_alias_entries table (V046)
CREATE TABLE game_alias_entries (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    game_id BIGINT NOT NULL COMMENT '所属游戏 ID',
    alias VARCHAR(50) NOT NULL UNIQUE COMMENT '别名字符串（全局唯一）',
    INDEX idx_game_id (game_id),
    INDEX idx_alias (alias),
    CONSTRAINT fk_game_alias_game FOREIGN KEY (game_id) REFERENCES games(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='游戏别名条目表';
