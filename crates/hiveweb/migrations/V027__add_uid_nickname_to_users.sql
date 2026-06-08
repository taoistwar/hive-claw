-- V027__add_uid_nickname_to_users
-- 记录外部用户（cloud_user）的 SaaS UID 和昵称

ALTER TABLE users
    ADD COLUMN uid VARCHAR(64) DEFAULT NULL COMMENT '外部用户SaaS UID' AFTER id,
    ADD COLUMN nickname VARCHAR(128) DEFAULT NULL COMMENT '用户昵称' AFTER uid;
