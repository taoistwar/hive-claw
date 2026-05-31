// 004 Agent Runtime — reactflow + Monaco 公共配置（T036）
//
// 集中维护 DAG 编辑器与代码编辑器的默认选项，避免散落在各页面。

import type { CSSProperties } from 'react';

// reactflow 画布默认尺寸
export const DEFAULT_FLOW_STYLE: CSSProperties = {
  width: '100%',
  height: 'calc(100vh - 200px)',
  background: '#fafafa',
};

// reactflow 默认 fitView padding
export const DEFAULT_FIT_VIEW_OPTIONS = { padding: 0.2 } as const;

// Monaco editor 默认选项（system_prompt / SchemaEditor / SkillMarkdownEditor 共享）
export const MONACO_DEFAULT_OPTIONS = {
  minimap: { enabled: false },
  fontSize: 14,
  lineNumbers: 'on' as const,
  scrollBeyondLastLine: false,
  wordWrap: 'on' as const,
  automaticLayout: true,
  tabSize: 2,
};

// DAG 节点类型常量
export const NODE_TYPE_FUNCTION = 'function';
export const NODE_TYPE_INPUT = 'input';
export const NODE_TYPE_OUTPUT = 'output';
