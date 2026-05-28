ALTER TABLE tools ADD COLUMN category_id BIGINT NULL AFTER output_schema;
ALTER TABLE tools ADD CONSTRAINT fk_tools_category FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE SET NULL;
