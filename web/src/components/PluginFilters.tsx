// PluginFilters — 全字段搜索 UI

import { Input, Space, Switch } from 'antd';
import type { PluginListParams } from '../services/plugin';

export interface PluginFiltersProps {
  value: PluginListParams;
  onChange: (next: PluginListParams) => void;
}

export function PluginFilters({ value, onChange }: PluginFiltersProps) {
  const apply = (patch: Partial<PluginListParams>) => onChange({ ...value, ...patch, offset: 0 });

  return (
    <Space wrap aria-label="Plugin 全字段检索">
      <Input.Search
        placeholder="搜索 name / description / identifier"
        allowClear
        value={value.search ?? ''}
        onChange={(e) => apply({ search: e.target.value || undefined })}
        onSearch={(v) => apply({ search: v || undefined })}
        style={{ width: 280 }}
        aria-label="搜索关键字"
      />
      <Input
        placeholder="identifier"
        value={value.identifier ?? ''}
        onChange={(e) => apply({ identifier: e.target.value || undefined })}
        allowClear
        style={{ width: 160 }}
        aria-label="按 identifier 筛选"
      />
      <Input
        placeholder="name"
        value={value.name ?? ''}
        onChange={(e) => apply({ name: e.target.value || undefined })}
        allowClear
        style={{ width: 140 }}
        aria-label="按 name 筛选"
      />
      <Input
        placeholder="version"
        value={value.version ?? ''}
        onChange={(e) => apply({ version: e.target.value || undefined })}
        allowClear
        style={{ width: 120 }}
        aria-label="按 version 筛选"
      />
      <Input
        placeholder="runtime"
        value={value.runtime ?? ''}
        onChange={(e) => apply({ runtime: e.target.value || undefined })}
        allowClear
        style={{ width: 120 }}
        aria-label="按 runtime 筛选"
      />
      <Input
        placeholder="author"
        value={value.author ?? ''}
        onChange={(e) => apply({ author: e.target.value || undefined })}
        allowClear
        style={{ width: 140 }}
        aria-label="按 author 筛选"
      />
      <Input
        placeholder="repository_url"
        value={value.repository_url ?? ''}
        onChange={(e) => apply({ repository_url: e.target.value || undefined })}
        allowClear
        style={{ width: 200 }}
        aria-label="按 repository_url 筛选"
      />
      <Input
        placeholder="description"
        value={value.description ?? ''}
        onChange={(e) => apply({ description: e.target.value || undefined })}
        allowClear
        style={{ width: 200 }}
        aria-label="按 description 筛选"
      />
      <Input
        placeholder="tag ids (逗号分隔)"
        defaultValue={value.tag_ids?.join(',')}
        onBlur={(e) => {
          const ids = e.target.value
            .split(',')
            .map((s) => Number(s.trim()))
            .filter((n) => !Number.isNaN(n) && n > 0);
          apply({ tag_ids: ids.length ? ids : undefined });
        }}
        style={{ width: 180 }}
        aria-label="按 tag 筛选"
      />
      <Space>
        <span style={{ fontSize: 14 }}>回收站</span>
        <Switch
          checked={!!value.deleted_only}
          onChange={(checked) => apply({ deleted_only: checked })}
          aria-label="是否显示已删除的 Plugin"
        />
      </Space>
    </Space>
  );
}
