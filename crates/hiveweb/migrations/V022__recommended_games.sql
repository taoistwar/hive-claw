CREATE TABLE IF NOT EXISTS recommended_games (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    name VARCHAR(255) NOT NULL COMMENT '推荐名称',
    reply TEXT NOT NULL COMMENT '回复内容',
    game_id VARCHAR(128) NOT NULL COMMENT '游戏ID',
    game_name VARCHAR(255) NOT NULL COMMENT '游戏名称',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    UNIQUE KEY uk_game_id (game_id),
    INDEX idx_name (name),
    INDEX idx_created_at (created_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='推荐游戏管理表';
