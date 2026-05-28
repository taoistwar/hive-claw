ALTER TABLE workflows ADD COLUMN category_id BIGINT NULL COMMENT '所属分类' AFTER required_capabilities;
ALTER TABLE workflows ADD CONSTRAINT fk_workflows_category FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE SET NULL;
