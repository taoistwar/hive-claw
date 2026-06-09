CREATE TABLE IF NOT EXISTS recommended_games_strategy (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    recommended_game_id BIGINT NOT NULL,
    channel JSON NOT NULL COMMENT '渠道列表（JSON数组，* 表示全部）',
    client_type JSON NOT NULL COMMENT '客户端类型列表（JSON数组，* 表示全部）',
    strategy VARCHAR(16) NOT NULL DEFAULT 'INCLUDE' COMMENT '策略：INCLUDE 包含 / EXCLUDE 排除',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    FOREIGN KEY (recommended_game_id) REFERENCES recommended_games(id) ON DELETE CASCADE,
    INDEX idx_rs_recommended_game_id (recommended_game_id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='推荐游戏策略表';
