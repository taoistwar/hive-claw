-- Rename card_content -> reason
ALTER TABLE recommended_games CHANGE COLUMN card_content reason TEXT COMMENT '推荐理由';

-- Add new fields
ALTER TABLE recommended_games
    ADD COLUMN tag VARCHAR(32) COMMENT '标签：运营推荐/新游上线/本周热玩' AFTER reason,
    ADD COLUMN game_category VARCHAR(64) COMMENT '游戏类型' AFTER tag,
    ADD COLUMN game_image VARCHAR(512) COMMENT '推荐图片地址' AFTER game_category;
