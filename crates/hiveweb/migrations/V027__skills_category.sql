ALTER TABLE skills ADD COLUMN category_id BIGINT NULL AFTER source;
ALTER TABLE skills ADD CONSTRAINT fk_skills_category FOREIGN KEY (category_id) REFERENCES categories(id) ON DELETE SET NULL;
