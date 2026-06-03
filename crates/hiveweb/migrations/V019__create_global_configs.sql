-- 全局配置表
CREATE TABLE IF NOT EXISTS global_configs (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    name VARCHAR(64) NOT NULL COMMENT '配置名称',
    `key` VARCHAR(64) NOT NULL UNIQUE COMMENT '配置键',
    type VARCHAR(32) NOT NULL COMMENT '配置值类型（string/number/boolean/json）',
    data JSON NOT NULL COMMENT '配置值',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='全局配置表';

-- 默认配置项
INSERT INTO global_configs (name, `key`, type, data) VALUES
    ('非会员次数', 'normal_ask_times', 'number', JSON_OBJECT('value', 5)),
    ('会员次数', 'vip_ask_times', 'number', JSON_OBJECT('value', 50));
