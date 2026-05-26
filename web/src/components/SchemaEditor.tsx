// SchemaEditor — Monaco JSON 编辑 + 实时校验 (T090)
//
// 当前 stub：使用 antd Input.TextArea + onChange JSON 实时校验。Monaco 替换会在
// /agents/system_prompt 页面落地后统一引入 @monaco-editor/react，避免单独引入
// Monaco worker 配置在多处重复。

import { useState } from 'react';
import { Input, Typography } from 'antd';

const { Text } = Typography;

export interface SchemaEditorProps {
  value: unknown;
  onChange: (next: unknown) => void;
  label?: string;
  rows?: number;
}

export function SchemaEditor({ value, onChange, label, rows = 8 }: SchemaEditorProps) {
  const [text, setText] = useState<string>(() => JSON.stringify(value ?? {}, null, 2));
  const [err, setErr] = useState<string>('');

  const onTextChange = (next: string) => {
    setText(next);
    try {
      const parsed = JSON.parse(next);
      onChange(parsed);
      setErr('');
    } catch (e) {
      setErr((e as Error).message);
    }
  };

  return (
    <div aria-label={label ?? 'JSON Schema 编辑器'}>
      {label ? (
        <Text strong style={{ display: 'block', marginBottom: 4 }}>
          {label}
        </Text>
      ) : null}
      <Input.TextArea
        rows={rows}
        value={text}
        onChange={(e) => onTextChange(e.target.value)}
        spellCheck={false}
        style={{ fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace' }}
        aria-invalid={!!err}
      />
      {err ? (
        <Text type="danger" role="alert" style={{ display: 'block', marginTop: 4 }}>
          JSON 解析错误：{err}
        </Text>
      ) : null}
    </div>
  );
}
