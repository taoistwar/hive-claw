// PluginFilters — search + category + tags 三维检索 UI (T076)
//
// 当前 stub：仅 search + category_id + 逗号分隔 tag_ids 输入。
// US7 接入 Category / Tag CRUD 后替换为树形 Select / 多选标签。

import { Input, InputNumber, Space } from 'antd';
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
      <InputNumber
        placeholder="category_id"
        min={1}
        value={value.category_id}
        onChange={(v) =>
          onChange({ ...value, category_id: v === null ? undefined : Number(v), offset: 0 })
        }
        aria-label="按 category 筛选"
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
    </Space>
  );
}
