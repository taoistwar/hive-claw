// PluginFilters — search + tags + deleted_only 三维检索 UI (T076)

import { Input, Space, Switch } from 'antd';
import type { PluginListParams } from '../services/plugin';

export interface PluginFiltersProps {
  value: PluginListParams;
  onChange: (next: PluginListParams) => void;
}

export function PluginFilters({ value, onChange }: PluginFiltersProps) {
  return (
    <Space wrap aria-label="Plugin 三维检索">
      <Input.Search
        placeholder="搜索 name / description / identifier"
        allowClear
        defaultValue={value.search}
        onSearch={(v) => onChange({ ...value, search: v || undefined, offset: 0 })}
        style={{ width: 320 }}
        aria-label="搜索关键字"
      />
      <Input
        placeholder="tag ids (逗号分隔)"
        defaultValue={value.tag_ids?.join(',')}
        onBlur={(e) => {
          const ids = e.target.value
            .split(',')
            .map((s) => Number(s.trim()))
            .filter((n) => !Number.isNaN(n) && n > 0);
          onChange({ ...value, tag_ids: ids.length ? ids : undefined, offset: 0 });
        }}
        style={{ width: 220 }}
        aria-label="按 tag 筛选"
      />
      <Space>
        <span style={{ fontSize: 14 }}>回收站</span>
        <Switch
          checked={!!value.deleted_only}
          onChange={(checked) => onChange({ ...value, deleted_only: checked, offset: 0 })}
          aria-label="是否显示已删除的 Plugin"
        />
      </Space>
    </Space>
  );
}
