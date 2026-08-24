INSERT INTO
  `hiveweb`.`functions` (
    `identifier`,
    `name`,
    `description`,
    `kind`,
    `input_schema`,
    `output_schema`,
    `plugin_id`,
    `plugin_export`,
    `category_id`,
    `required_capabilities`,
    `created_at`,
    `updated_at`
  )
VALUES
  (
    'rag_answer',
    'RAG Answer',
    '检索 RAGflow 知识库并结合 LLM 生成回复；无匹配时先说明无相关知识后再给出备选回答。',
    1,
    '{"type": "object", "properties": {}}',
    '{"type": "object", "required": [], "properties": {"message": {"type": "string", "description": "最终回复文本"}}}',
    NULL,
    NULL,
    NULL,
    '[]',
    '2026-08-05 08:00:21',
    '2026-08-06 08:02:40'
  );

-- 这里的 function_id 9362 对应上面的函数，workflow_id 为 NULL 表示该工具不依赖于特定的工作流，可以直接使用。
INSERT INTO
  `hiveweb`.`tools` (
    `identifier`,
    `name`,
    `description`,
    `kind`,
    `source`,
    `is_always`,
    `function_id`,
    `workflow_id`,
    `input_schema`,
    `output_schema`,
    `category_id`,
    `required_capabilities`,
    `created_at`,
    `updated_at`
  )
VALUES
  (
    'rag_answer',
    'RAG Answer',
    '检索 RAGflow 知识库并结合 LLM 生成回复；无匹配时先说明无相关知识后再给出备选回答。',
    1,
    'builtin',
    0,
    -- 26,
    NULL,
    '{"type": "object", "properties": {}}',
    '{"type": "object", "required": [], "properties": {"message": {"type": "string", "description": "最终回复文本"}}}',
    NULL,
    '[]',
    '2026-08-05 08:00:22',
    '2026-08-06 08:02:40'
  );

-- 这里的 agent_id 1 对应默认的 agent，tool_id 83 对应上面的工具。
INSERT INTO
  `hiveweb`.`agent_tools` (`agent_id`, `tool_id`)
VALUES
  (
    1,
    83
  );


