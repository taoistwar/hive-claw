ALTER TABLE recommended_games MODIFY COLUMN game_category JSON COMMENT '游戏类型（JSON 数组，每项包含 name 和 type）';
