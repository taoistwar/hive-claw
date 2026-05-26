-- V003__seed_super_admin（占位 / no-op）
--
-- 按 data-model.md §Migrations 的约定，初始超级管理员不直接通过 SQL 写入，
-- 而是由 `cargo run --bin create_super_admin -- --phone ... --password ...`
-- 命令使用 bcrypt 加密后插入。
--
-- 本文件保留版本号占位，避免后续部署在版本序列上出现编号断层。
SELECT 1;
