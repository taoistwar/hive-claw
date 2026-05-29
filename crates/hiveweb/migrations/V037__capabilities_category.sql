ALTER TABLE capabilities ADD COLUMN category_id BIGINT NULL COMMENT '所属分类' AFTER is_dangerous;
ALTER TABLE capabilities ADD CONSTRAINT fk_capabilities_category FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE SET NULL;
