import { useEffect, useState } from 'react';
import { Select, message } from 'antd';
import { listTags, type TagItem } from '../services/tag';

export interface TagMultiSelectProps {
  value?: number[];
  onChange?: (value: number[]) => void;
}

export function TagMultiSelect({ value, onChange }: TagMultiSelectProps) {
  const [tags, setTags] = useState<TagItem[]>([]);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    listTags()
      .then((res) => {
        if (!cancelled) setTags(res.items);
      })
      .catch((e) => {
        if (!cancelled) {
          void message.error(`加载标签失败：${(e as Error).message}`);
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <Select<number[]>
      mode="multiple"
      allowClear
      showSearch
      placeholder="选择标签（可多选）"
      value={value ?? []}
      onChange={onChange}
      loading={loading}
      filterOption={(input, option) =>
        String(option?.label ?? '')
          .toLowerCase()
          .includes(input.toLowerCase())
      }
      options={tags.map((t) => ({
        label: t.name,
        value: t.id,
      }))}
    />
  );
}
