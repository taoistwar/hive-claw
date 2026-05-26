// ToolPage — Tool 列表 (T093)

import { useEffect, useState } from 'react';
import { Table, Tag, message } from 'antd';
import type { ColumnsType } from 'antd/es/table';
import { listTools, type ToolItem, type ToolList } from '../services/tool';

export default function ToolPage() {
  const [data, setData] = useState<ToolList>({ items: [], total: 0, offset: 0, limit: 20 });
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    setLoading(true);
    listTools({})
      .then(setData)
      .catch((e) => message.error(`加载失败：${(e as Error).message}`))
      .finally(() => setLoading(false));
  }, []);

  const columns: ColumnsType<ToolItem> = [
    { title: 'ID', dataIndex: 'id', width: 60 },
    { title: 'identifier', dataIndex: 'identifier' },
    { title: 'name', dataIndex: 'name' },
    {
      title: 'kind',
      dataIndex: 'kind',
      width: 140,
      render: (k: number) =>
        k === 1 ? <Tag color="green">function-wrap</Tag> : <Tag color="cyan">workflow-wrap</Tag>,
    },
    { title: 'function_id', dataIndex: 'function_id' },
    { title: 'workflow_id', dataIndex: 'workflow_id' },
  ];

  return (
    <Table<ToolItem>
      rowKey="id"
      columns={columns}
      dataSource={data.items}
      loading={loading}
      pagination={{ total: data.total, pageSize: data.limit }}
    />
  );
}
