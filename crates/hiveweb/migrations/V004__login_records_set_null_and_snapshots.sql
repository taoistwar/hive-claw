-- T096 / data-model.md §LoginRecord 修订（2026-05-26）
-- 把 fk_login_admin 由 CASCADE 改为 SET NULL，并新增审计快照列。
-- 这样删除管理员时仍保留登录历史以满足 spec FR-022 的 90 天保留要求。

ALTER TABLE login_records
    DROP FOREIGN KEY fk_login_admin;

ALTER TABLE login_records
    MODIFY COLUMN admin_id BIGINT NULL,
    ADD COLUMN admin_phone_snapshot VARCHAR(11) NOT NULL DEFAULT '' AFTER admin_id,
    ADD COLUMN admin_nickname_snapshot VARCHAR(20) NOT NULL DEFAULT '' AFTER admin_phone_snapshot;

ALTER TABLE login_records
    ADD CONSTRAINT fk_login_admin
    FOREIGN KEY (admin_id) REFERENCES admins(id) ON DELETE SET NULL;

ALTER TABLE login_records
    COMMENT = '登录记录表（审计用，保留 90+ 天，admin 删除后 admin_id 置 NULL）';
