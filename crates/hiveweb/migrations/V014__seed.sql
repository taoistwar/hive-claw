-- 1. main Agent（不可删除，service 层强制）
INSERT IGNORE INTO agents (id, identifier, name, description, system_prompt, parent_agent_id, depth)
VALUES (1, 'main', '入口 Agent', '系统的入口 Agent；不可删除',
        'You are the main entry agent. Decide whether to answer directly or route to a sub-agent.',
        NULL, 0);

-- 2. capabilities 元数据
INSERT IGNORE INTO capabilities (name, description, is_dangerous) VALUES
  ('network.http',  'HTTP/HTTPS access (allowlisted hosts, SSRF-blocked)', 1),
  ('fs.read',       '/tmp/plugin/ 内文件读', 0),
  ('fs.write',      '/tmp/plugin/ 内文件写', 0),
  ('s3.read',       'Rustfs 桶 GET', 0),
  ('s3.write',      'Rustfs 桶 PUT / DELETE', 0),
  ('db.query',      '宿主预注册命名 SELECT 查询', 0),
  ('db.execute',    '宿主预注册命名 DML（永不自由 SQL）', 1),
  ('llm.invoke',    'LLM 调用（走 Agent.model_preset 解析）', 0),
  ('secret.get',    'allowlist 内的密钥读取', 1),
  ('time.now',      '服务器当前时间', 0),
  ('log.emit',      '结构化日志写入（rate-limited）', 0),
  ('chat.respond',  '提交 Agent 最终用户可见回复', 0),
  ('exec.run',      'Shell 命令执行（受 workspace 边界约束）', 1),
  ('agent.spawn',   '生成子 Agent 执行独立任务', 0),
  ('cron.manage',   '管理定时 Cron 任务', 0);
