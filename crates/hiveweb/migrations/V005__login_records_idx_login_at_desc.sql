-- T097 / data-model.md §LoginRecord 修订（2026-05-26）
-- 把 idx_login_at 改为 DESC，加速仪表盘 `ORDER BY login_at DESC LIMIT 10`。
-- MySQL 8.0+ 支持 descending indexes（B+-tree 反向遍历）。

ALTER TABLE login_records
    DROP INDEX idx_login_at,
    ADD INDEX idx_login_at (login_at DESC);
