-- Seed default color tags
INSERT INTO tags (name, color) VALUES
    ('红', '#EF4444'),
    ('橙', '#F97316'),
    ('黄', '#EAB308'),
    ('绿', '#22C55E'),
    ('蓝', '#3B82F6'),
    ('紫', '#A855F7'),
    ('灰', '#6B7280')
ON DUPLICATE KEY UPDATE color = VALUES(color);
