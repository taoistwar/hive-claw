// FunctionDetail — read-only function information viewer (T092 扩展)

import { Button, Descriptions, Tag } from 'antd';
import type { FunctionItem } from '../services/function';

export interface FunctionDetailProps {
  fn: FunctionItem;
  onEdit?: () => void;
  onBack?: () => void;
}

export function FunctionDetail({ fn, onEdit, onBack }: FunctionDetailProps) {
  return (
    <div>
      <Descriptions bordered column={1} title="函数详情" size="small">
        <Descriptions.Item label="ID">{fn.id}</Descriptions.Item>
        <Descriptions.Item label="identifier">
          <code>{fn.identifier}</code>
        </Descriptions.Item>
        <Descriptions.Item label="name">{fn.name}</Descriptions.Item>
        <Descriptions.Item label="kind">
          {fn.kind === 1 ? (
            <Tag color="purple">builtin</Tag>
          ) : (
            <Tag color="blue">custom</Tag>
          )}
        </Descriptions.Item>
        <Descriptions.Item label="description">
          {fn.description || '—'}
        </Descriptions.Item>
        <Descriptions.Item label="plugin">
          {fn.plugin_identifier ? (
            <>
              <code>{fn.plugin_identifier}</code>{' '}
              <span style={{ color: '#999' }}>(id={fn.plugin_id})</span>
            </>
          ) : (
            '—'
          )}
        </Descriptions.Item>
        <Descriptions.Item label="plugin_export">
          {fn.plugin_export || '—'}
        </Descriptions.Item>
        <Descriptions.Item label="category">
          {fn.category_id ? `ID: ${fn.category_id}` : '—'}
        </Descriptions.Item>
        <Descriptions.Item label="input_schema">
          <pre
            style={{
              fontSize: 12,
              margin: 0,
              maxHeight: 200,
              overflow: 'auto',
              background: '#fafafa',
              padding: 8,
            }}
          >
            {JSON.stringify(fn.input_schema, null, 2)}
          </pre>
        </Descriptions.Item>
        <Descriptions.Item label="output_schema">
          <pre
            style={{
              fontSize: 12,
              margin: 0,
              maxHeight: 200,
              overflow: 'auto',
              background: '#fafafa',
              padding: 8,
            }}
          >
            {JSON.stringify(fn.output_schema, null, 2)}
          </pre>
        </Descriptions.Item>
        <Descriptions.Item label="created_at">
          {new Date(fn.created_at).toLocaleString('zh-CN')}
        </Descriptions.Item>
        <Descriptions.Item label="updated_at">
          {new Date(fn.updated_at).toLocaleString('zh-CN')}
        </Descriptions.Item>
      </Descriptions>

      <div style={{ marginTop: 16, display: 'flex', gap: 8 }}>
        {onBack && <Button onClick={onBack}>返回</Button>}
        {onEdit && <Button type="primary" onClick={onEdit}>编辑</Button>}
      </div>
    </div>
  );
}
