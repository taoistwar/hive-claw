-- Hive Metastore MySQL 测试数据
-- 用于测试元数据服务连接

-- 创建测试数据库
CREATE DATABASE IF NOT EXISTS hive_test;
USE hive_test;

-- 创建测试表（模拟 Hive Metastore 的 DBS 表）
CREATE TABLE IF NOT EXISTS DBS (
    DB_ID bigint PRIMARY KEY,
    NAME varchar(128) UNIQUE,
    DB_LOCATION_URI varchar(4000),
    OWNER_NAME varchar(128),
    OWNER_TYPE varchar(128),
    CTLG_NAME varchar(128) DEFAULT 'hive'
);

-- 创建测试表（模拟 Hive Metastore 的 TBLS 表）
CREATE TABLE IF NOT EXISTS TBLS (
    TBL_ID bigint PRIMARY KEY,
    TBL_NAME varchar(128),
    DB_ID bigint,
    OWNER varchar(768),
    TBL_TYPE varchar(128),
    CREATE_TIME int DEFAULT 0,
    LAST_ACCESS_TIME int DEFAULT 0,
    RETENTION int DEFAULT 0,
    SD_ID bigint,
    VIEW_EXPANDED_TEXT mediumtext,
    VIEW_ORIGINAL_TEXT mediumtext,
    IS_REWRITE_ENABLED bit DEFAULT b'0',
    FOREIGN KEY (DB_ID) REFERENCES DBS(DB_ID)
);

-- 插入测试数据
INSERT INTO DBS (DB_ID, NAME, DB_LOCATION_URI, OWNER_NAME) VALUES
(1, 'default', 'hdfs://localhost:9000/warehouse/default', 'admin'),
(2, 'ods', 'hdfs://localhost:9000/warehouse/ods', 'admin'),
(3, 'dwd', 'hdfs://localhost:9000/warehouse/dwd', 'admin');

SELECT 'Test data created successfully!' as status;
