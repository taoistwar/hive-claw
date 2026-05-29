ALTER TABLE workflows ADD COLUMN input_schema JSON NULL COMMENT '工作流起始节点的输入变量定义 (JSON Schema format)' AFTER category_id;
ALTER TABLE workflows ADD COLUMN start_description VARCHAR(512) NULL COMMENT '起始节点描述/欢迎语' AFTER input_schema;
