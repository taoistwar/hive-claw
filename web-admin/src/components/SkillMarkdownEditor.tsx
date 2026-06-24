// SkillMarkdownEditor — Monaco markdown editor + frontmatter (T091)
//
// 拆分 frontmatter (YAML) 和 markdown content。frontmatter 用 JSON 文本编辑
// （保持与 SchemaEditor 一致的 textarea），content 用 Monaco markdown 模式。

import { useState } from 'react';
import Editor from '@monaco-editor/react';
import { Input, Typography } from 'antd';
import { MONACO_DEFAULT_OPTIONS } from '../utils/reactflow';

const { Text } = Typography;

const MAX_CONTENT_BYTES = 64 * 1024;

export interface SkillMarkdownEditorProps {
  content: string;
  frontmatter: unknown | null;
  onContentChange: (next: string) => void;
  onFrontmatterChange: (next: unknown | null) => void;
}

export function SkillMarkdownEditor({
  content,
  frontmatter,
  onContentChange,
  onFrontmatterChange,
}: SkillMarkdownEditorProps) {
  const [frontmatterText, setFrontmatterText] = useState<string>(() =>
    frontmatter ? JSON.stringify(frontmatter, null, 2) : '',
  );
  const [fmError, setFmError] = useState<string>('');

  const onFmTextChange = (next: string) => {
    setFrontmatterText(next);
    if (!next.trim()) {
      onFrontmatterChange(null);
      setFmError('');
      return;
    }
    try {
      onFrontmatterChange(JSON.parse(next));
      setFmError('');
    } catch (e) {
      setFmError((e as Error).message);
    }
  };

  const byteSize = new Blob([content]).size;
  const oversized = byteSize > MAX_CONTENT_BYTES;

  return (
    <div aria-label="Skill markdown 编辑器">
      <Text strong style={{ display: 'block', marginBottom: 4 }}>
        Frontmatter (JSON)
      </Text>
      <Input.TextArea
        rows={4}
        value={frontmatterText}
        onChange={(e) => onFmTextChange(e.target.value)}
        placeholder='{"tags": ["coding"], "version": "1.0"}'
        spellCheck={false}
        aria-label="frontmatter JSON 输入"
        aria-invalid={!!fmError}
        style={{ fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace' }}
      />
      {fmError ? (
        <Text type="danger" role="alert" style={{ display: 'block', marginTop: 4 }}>
          JSON 错误：{fmError}
        </Text>
      ) : null}

      <Text strong style={{ display: 'block', marginTop: 16, marginBottom: 4 }}>
        Content (Markdown){' '}
        <Text type={oversized ? 'danger' : 'secondary'} style={{ fontSize: 12 }}>
          {(byteSize / 1024).toFixed(1)} KB / 64 KB{oversized ? ' — 已超出' : ''}
        </Text>
      </Text>
      <div
        style={{ border: oversized ? '1px solid #ff4d4f' : '1px solid #d9d9d9', borderRadius: 4 }}
        aria-label="markdown 内容编辑器"
      >
        <Editor
          height="360px"
          language="markdown"
          value={content}
          onChange={(v) => onContentChange(v ?? '')}
          options={MONACO_DEFAULT_OPTIONS}
        />
      </div>
    </div>
  );
}
