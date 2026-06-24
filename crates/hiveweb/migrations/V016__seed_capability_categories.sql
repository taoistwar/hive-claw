-- 预置 Capability 内置分类（与 runtime/capability.rs 中的 CAPABILITIES 对应）
INSERT IGNORE INTO categories (parent_id, name, slug, description) VALUES
  (NULL, '网络', 'network', '网络相关能力'),
  (NULL, '文件系统', 'fs', '文件系统读写能力'),
  (NULL, '对象存储', 's3', '对象存储（Rustfs）能力'),
  (NULL, '数据库', 'db', '数据库查询与执行'),
  (NULL, 'LLM', 'llm', '大语言模型调用'),
  (NULL, '密钥管理', 'secret', '密钥读取与管理'),
  (NULL, '时间', 'time', '服务器时间相关'),
  (NULL, '日志', 'log', '日志写入'),
  (NULL, '聊天', 'chat', '聊天交互'),
  (NULL, '命令执行', 'exec', 'Shell 命令执行'),
  (NULL, 'Agent', 'agent', 'Agent 管理'),
  (NULL, '定时任务', 'cron', 'Cron 任务管理');

-- 更新 capabilities 的 category_id（按 slug 匹配）
UPDATE capabilities c
JOIN categories cat ON cat.slug = SUBSTRING_INDEX(c.name, '.', 1) AND cat.parent_id IS NULL
SET c.category_id = cat.id
WHERE cat.slug IN ('network', 'fs', 's3', 'db', 'llm', 'secret', 'time', 'log', 'chat', 'exec', 'agent', 'cron');
