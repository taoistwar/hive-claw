ALTER TABLE recommended_games ADD COLUMN sort_value INT NOT NULL DEFAULT 0 COMMENT '排序值（数值越小越靠前）' AFTER card_content;
ALTER TABLE recommended_games ADD INDEX idx_sort_value (sort_value);
