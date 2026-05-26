-- T100：admin 列表查询 `ORDER BY created_at DESC LIMIT 10` 默认走 filesort
-- 全表扫。在 100+ 管理员（SC-005）下尚可，但留余地给后续扩张。
ALTER TABLE admins ADD INDEX idx_created_at (created_at DESC);
