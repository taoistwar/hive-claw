// SystemPromptEditor — Monaco editor for Agent.system_prompt (T124 部分)
//
// Agent 的 system_prompt 通常较长（含人设、规则、工具使用说明等），
// 用 Monaco 而非 textarea 提供更舒适的多行编辑体验。

import Editor from '@monaco-editor/react';
import { Typography } from 'antd';
import { MONACO_DEFAULT_OPTIONS } from '../utils/reactflow';

const { Text } = Typography;

export interface SystemPromptEditorProps {
  value: string;
  onChange: (next: string) => void;
  height?: string | number;
}

export function SystemPromptEditor({
  value,
  onChange,
  height = '320px',
}: SystemPromptEditorProps) {
  return (
    <div aria-label="Agent system_prompt 编辑器">
      <Text type="secondary" style={{ fontSize: 12, display: 'block', marginBottom: 4 }}>
        Monaco editor — 支持多行、缩进、查找替换 (Ctrl/Cmd+F)
      </Text>
      <div style={{ border: '1px solid #d9d9d9', borderRadius: 4 }}>
        <Editor
          height={height}
          language="plaintext"
          value={value}
          onChange={(v) => onChange(v ?? '')}
          options={{
            ...MONACO_DEFAULT_OPTIONS,
            wordWrap: 'on',
          }}
        />
      </div>
    </div>
  );
}
