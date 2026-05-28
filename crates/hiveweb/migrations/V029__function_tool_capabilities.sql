ALTER TABLE functions ADD COLUMN required_capabilities JSON NULL COMMENT '声明该 function 执行所需的 capabilities';
ALTER TABLE tools ADD COLUMN required_capabilities JSON NULL COMMENT '声明该 tool 执行所需的 capabilities';
