ALTER TABLE workflows ADD COLUMN output_schema JSON NULL COMMENT '工作流结束节点的输出变量定义 (JSON Schema format)' AFTER start_description;
ALTER TABLE workflows ADD COLUMN end_description VARCHAR(512) NULL COMMENT '结束节点描述/结束语' AFTER output_schema;
