// SchemaEditor — JSON Schema 编辑器 + 实时校验 (T090)
//
// 当前 stub：使用 antd Input.TextArea + onChange JSON 实时校验。Monaco 替换会在
// /agents/system_prompt 页面落地后统一引入 @monaco-editor/react，避免单独引入
// Monaco worker 配置在多处重复。

import { useState, useMemo } from 'react';
import { Input, Typography } from 'antd';

const { Text } = Typography;

export interface SchemaEditorProps {
  value: unknown;
  onChange: (next: unknown) => void;
  label?: string;
  rows?: number;
}

/** 对解析后的 JSON 做 JSON Schema 常见错误检查 */
function checkSchemaIssues(obj: unknown): string[] {
  const issues: string[] = [];
  if (!obj || typeof obj !== 'object' || Array.isArray(obj)) {
    issues.push('Schema 必须是 JSON 对象，不能是数组、字符串或原始值。');
    return issues;
  }
  const o = obj as Record<string, unknown>;

  // 1. 必须有 type
  if (!('type' in o) || typeof o.type !== 'string') {
    issues.push('缺少顶层 type 字段。JSON Schema 必须包含 type（如 "object"、"string" 等）。');
  }

  // 2. required 必须是数组，不能是布尔值
  if ('required' in o) {
    const r = o.required;
    if (typeof r === 'boolean') {
      issues.push('required 应为字符串数组（例如 ["field1", "field2"]），不能是 true/false。若顶层 type 为 string/number 等基础类型，请移除 required 字段。');
    } else if (!Array.isArray(r)) {
      issues.push('required 应为字符串数组，例如 ["field1", "field2"]。');
    }
  }

  // 3. type=object 时建议有 properties
  if (o.type === 'object' && !('properties' in o)) {
    issues.push('type 为 "object" 时建议包含 properties 字段描述对象属性。');
  }

  return issues;
}

export function SchemaEditor({ value, onChange, label, rows = 8 }: SchemaEditorProps) {
  const [text, setText] = useState<string>(() => JSON.stringify(value ?? {}, null, 2));
  const [err, setErr] = useState<string>('');
  const [parsed, setParsed] = useState<unknown>(value);

  const schemaIssues = useMemo(() => checkSchemaIssues(parsed), [parsed]);

  const onTextChange = (next: string) => {
    setText(next);
    try {
      const p = JSON.parse(next);
      setParsed(p);
      onChange(p);
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
        aria-invalid={!!err || schemaIssues.length > 0}
        aria-label={label ? `${label} (JSON)` : 'JSON 编辑器输入区'}
      />
      {err ? (
        <Text type="danger" role="alert" style={{ display: 'block', marginTop: 4 }}>
          JSON 解析错误：{err}
        </Text>
      ) : null}
      {!err && schemaIssues.length > 0
        ? schemaIssues.map((issue, i) => (
            <Text
              key={i}
              type="warning"
              role="alert"
              style={{ display: 'block', marginTop: i === 0 ? 4 : 2 }}
            >
              ⚠ {issue}
            </Text>
          ))
        : null}
    </div>
  );
}
