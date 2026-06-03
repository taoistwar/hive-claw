CREATE TABLE IF NOT EXISTS recommended_games (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    name VARCHAR(255) NOT NULL COMMENT '推荐名称',
    reply TEXT NOT NULL COMMENT '回复内容',
    reason TEXT COMMENT '推荐理由',
    tag VARCHAR(32) COMMENT '标签：运营推荐/新游上线/本周热玩',
    game_category VARCHAR(64) COMMENT '游戏类型',
    game_image VARCHAR(512) COMMENT '推荐图片地址',
    sort_value INT NOT NULL DEFAULT 0 COMMENT '排序值（数值越小越靠前）',
    game_id VARCHAR(128) NOT NULL COMMENT '游戏ID',
    game_name VARCHAR(255) NOT NULL COMMENT '游戏名称',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    UNIQUE KEY uk_game_id (game_id),
    INDEX idx_name (name),
    INDEX idx_sort_value (sort_value),
    INDEX idx_created_at (created_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='推荐游戏管理表';
