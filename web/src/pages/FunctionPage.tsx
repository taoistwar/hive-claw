// FunctionPage — 列表（区分 builtin / custom）(T092)

import { useEffect, useState } from 'react';
import { Table, Tag, Space, Input, Select, message } from 'antd';
import type { ColumnsType } from 'antd/es/table';
import { listFunctions, type FunctionItem, type FunctionList } from '../services/function';

export default function FunctionPage() {
  const [data, setData] = useState<FunctionList>({ items: [], total: 0, offset: 0, limit: 20 });
  const [loading, setLoading] = useState(false);
  const [search, setSearch] = useState('');
  const [kind, setKind] = useState<'builtin' | 'custom' | ''>('');

  useEffect(() => {
    setLoading(true);
    listFunctions({ search: search || undefined, kind: kind || undefined })
      .then(setData)
      .catch((e) => message.error(`加载失败：${(e as Error).message}`))
      .finally(() => setLoading(false));
  }, [search, kind]);

  const columns: ColumnsType<FunctionItem> = [
    { title: 'ID', dataIndex: 'id', width: 60 },
    { title: 'identifier', dataIndex: 'identifier' },
    { title: 'name', dataIndex: 'name' },
    {
      title: 'kind',
      dataIndex: 'kind',
      width: 100,
      render: (k: number) =>
        k === 1 ? <Tag color="purple">builtin</Tag> : <Tag color="blue">custom</Tag>,
    },
    { title: 'plugin', dataIndex: 'plugin_identifier', render: (v?: string | null) => v ?? '-' },
    { title: 'plugin_export', dataIndex: 'plugin_export' },
  ];

  return (
    <div>
      <Space style={{ marginBottom: 16 }}>
        <Input.Search
          placeholder="搜索 function"
          allowClear
          onSearch={setSearch}
          style={{ width: 280 }}
          aria-label="搜索 function"
        />
        <Select
          value={kind}
          onChange={(v) => setKind(v)}
          style={{ width: 140 }}
          aria-label="按 kind 筛选"
          options={[
            { value: '', label: '全部 kind' },
            { value: 'builtin', label: 'builtin' },
            { value: 'custom', label: 'custom' },
          ]}
        />
      </Space>
      <Table<FunctionItem>
        rowKey="id"
        columns={columns}
        dataSource={data.items}
        loading={loading}
        pagination={{ total: data.total, pageSize: data.limit }}
      />
    </div>
  );
}
