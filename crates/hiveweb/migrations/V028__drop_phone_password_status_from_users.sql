-- V028__drop_phone_password_status_from_users
-- 删除 phone/password_hash/status，外部用户信息由 uid/nickname 承载

ALTER TABLE users
    DROP COLUMN phone,
    DROP COLUMN password_hash,
    DROP COLUMN status,
    DROP INDEX idx_users_phone;
