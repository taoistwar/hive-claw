ALTER TABLE workflows ADD COLUMN required_capabilities JSON NULL COMMENT '计算出的执行所需 capabilities（由 DAG 中所有节点的 function 聚合）';
