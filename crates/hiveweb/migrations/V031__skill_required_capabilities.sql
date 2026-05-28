ALTER TABLE skills ADD COLUMN required_capabilities JSON NULL COMMENT '声明该 skill 执行所需的 capabilities（来源于它所调用的 function 或 workflow）';
