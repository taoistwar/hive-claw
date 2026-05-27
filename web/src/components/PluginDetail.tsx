// PluginDetail — read-only plugin information viewer

import { useEffect, useState } from 'react';
import { Button, Descriptions, Spin, Tag } from 'antd';
import type { Plugin, PluginTag } from '../services/plugin';
import { getPluginExports } from '../services/plugin';

export interface PluginDetailProps {
  plugin: Plugin;
  categoryNameMap?: Map<number, string>;
  onEdit?: () => void;
  onBack?: () => void;
}

export function PluginDetail({ plugin, categoryNameMap, onEdit, onBack }: PluginDetailProps) {
  const [exports, setExports] = useState<string[]>([]);
  const [exportsLoading, setExportsLoading] = useState(false);

  useEffect(() => {
    setExportsLoading(true);
    setExports([]);
    getPluginExports(plugin.id)
      .then((list) => setExports(list))
      .catch(() => {
        setExports([]);
      })
      .finally(() => setExportsLoading(false));
  }, [plugin.id]);

  const tagItems = plugin.tags?.map((t: PluginTag) => (
    <Tag key={t.id} color="blue">
      {t.name}
    </Tag>
  ));

  return (
    <div>
      <Descriptions
        bordered
        column={1}
        title="插件详情"
        size="small"
      >
        <Descriptions.Item label="ID">{plugin.id}</Descriptions.Item>
        <Descriptions.Item label="identifier">
          <code>{plugin.identifier}</code>
        </Descriptions.Item>
        <Descriptions.Item label="name">{plugin.name}</Descriptions.Item>
        <Descriptions.Item label="version">{plugin.version}</Descriptions.Item>
        <Descriptions.Item label="description">
          {plugin.description || '—'}
        </Descriptions.Item>
        <Descriptions.Item label="author">
          {plugin.author || '—'}
        </Descriptions.Item>
        <Descriptions.Item label="runtime">{plugin.runtime}</Descriptions.Item>
        <Descriptions.Item label="repository_url">
          {plugin.repository_url ? (
            <a href={plugin.repository_url} target="_blank" rel="noopener noreferrer">
              {plugin.repository_url}
            </a>
          ) : (
            '—'
          )}
        </Descriptions.Item>
        <Descriptions.Item label="category">
          {plugin.category_id ? (categoryNameMap?.get(plugin.category_id) ?? `ID: ${plugin.category_id}`) : '—'}
        </Descriptions.Item>
        <Descriptions.Item label="tags">
          {tagItems?.length ? tagItems : '—'}
        </Descriptions.Item>
        <Descriptions.Item label="size_bytes">
          {(plugin.size_bytes / 1024).toFixed(1)} KB
        </Descriptions.Item>
        <Descriptions.Item label="sha256">
          <code style={{ fontSize: 12, wordBreak: 'break-all' }}>{plugin.sha256}</code>
        </Descriptions.Item>
        <Descriptions.Item label="s3_key">
          <code style={{ fontSize: 12, wordBreak: 'break-all' }}>{plugin.s3_key}</code>
        </Descriptions.Item>
        <Descriptions.Item label="exports">
          {exportsLoading ? (
            <Spin size="small" />
          ) : exports.length > 0 ? (
            exports.map((fn) => (
              <Tag key={fn} color="geekblue">{fn}</Tag>
            ))
          ) : (
            '—'
          )}
        </Descriptions.Item>
        <Descriptions.Item label="created_at">
          {new Date(plugin.created_at).toLocaleString('zh-CN')}
        </Descriptions.Item>
        <Descriptions.Item label="updated_at">
          {new Date(plugin.updated_at).toLocaleString('zh-CN')}
        </Descriptions.Item>
        <Descriptions.Item label="deleted_at">
          {plugin.deleted_at ? (
            <Tag color="red">已删除</Tag>
          ) : (
            <Tag color="green">正常</Tag>
          )}
        </Descriptions.Item>
      </Descriptions>

      <div style={{ marginTop: 16, display: 'flex', gap: 8 }}>
        {onBack && <Button onClick={onBack}>返回</Button>}
        {onEdit && <Button type="primary" onClick={onEdit}>编辑</Button>}
      </div>
    </div>
  );
}
