-- ============================================================================
-- 生产环境手工初始化 RAGFlow 全局配置
-- ============================================================================
-- 在 hiveweb 启动逻辑中，initialize() 已会自动 ensure 这些 key（INSERT IGNORE）。
-- 但如果你需要在启动前/或不想依赖代码路径，可手工执行本脚本。
--
-- 说明：
--   * `key` 列有 UNIQUE 约束，使用 INSERT IGNORE，重复执行安全（已存在的行不会被覆盖）。
--   * `data` 形态为 { "value": ... }，与 web-admin 创建结构一致，代码解析时取 data["value"]。
--   * `value` 默认值来自 RagflowConfig::default()（ragflow_config.rs）。
--   * 如需指定真实 base_url / api_key，请在执行前替换下面对应两行的 "" 值。
--
-- 用法：
--   mysql -h <host> -u <user> -p <dbname> < scripts/seed_ragflow_global_config.sql
-- ============================================================================


INSERT INTO `global_configs` (`name`, `key`, `type`, `data`) VALUES
('RAGFlow Base URL', 'ragflow_base_url', 'string', CAST('{"value":"http://10.201.2.234:8899"}' AS JSON)),
('RAGFlow API Key', 'ragflow_api_key', 'string', CAST('{"value":"ragflow-VjZDUzNGFhY2U2YzExZWZiNjA3MDI0Mm"}' AS JSON)),
('RAGFlow Dataset IDs', 'ragflow_dataset_ids', 'json', CAST('{"value":["ff832b0e105f11f0aadf0242ac140002"]}' AS JSON)),
('RAGFlow Document IDs', 'ragflow_document_ids', 'json', CAST('{"value":["ff864be0105f11f0aadf0242ac140002"]}' AS JSON)),
('RAGFlow Page', 'ragflow_page', 'number', CAST('{"value":1}' AS JSON)),
('RAGFlow Page Size', 'ragflow_page_size', 'number', CAST('{"value":6}' AS JSON)),
('RAGFlow Similarity Threshold', 'ragflow_similarity_threshold', 'number', CAST('{"value":0.20000000298023224}' AS JSON)),
('RAGFlow Vector Similarity Weight', 'ragflow_vector_similarity_weight', 'number', CAST('{"value":0.30000001192092896}' AS JSON)),
('RAGFlow Top K', 'ragflow_top_k', 'number', CAST('{"value":10}' AS JSON)),
('RAGFlow Rerank ID', 'ragflow_rerank_id', 'string', CAST('{"value":""}' AS JSON)),
('RAGFlow Keyword', 'ragflow_keyword', 'boolean', CAST('{"value":true}' AS JSON)),
('RAGFlow Highlight', 'ragflow_highlight', 'boolean', CAST('{"value":false}' AS JSON)),
('RAGFlow Timeout Secs', 'ragflow_timeout_secs', 'number', CAST('{"value":30}' AS JSON));


-- 校验插入结果（可选）
-- SELECT `key`, type, data FROM global_configs WHERE `key` LIKE 'ragflow_%';
