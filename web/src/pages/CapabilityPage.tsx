import { useCallback, useEffect, useState } from 'react';
import { Button, Drawer, Input, Modal, Space, Table, Tag, message } from 'antd';
import type { ColumnsType } from 'antd/es/table';
import {
  listCapabilities,
  type CapabilityItem,
  type CapabilityDetail,
  getCapabilityDetail,
} from '../services/capability';

export default function CapabilityPage() {
  const [items, setItems] = useState<CapabilityItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [viewOpen, setViewOpen] = useState(false);
  const [viewingCapability, setViewingCapability] = useState<CapabilityDetail | null>(null);
  const [viewLoading, setViewLoading] = useState(false);
  const [search, setSearch] = useState('');

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const all = await listCapabilities();
      if (search) {
        const lower = search.toLowerCase();
        setItems(
          all.filter(
            (c) =>
              c.name.toLowerCase().includes(lower) ||
              c.description.toLowerCase().includes(lower),
          ),
        );
      } else {
        setItems(all);
      }
    } catch (e) {
      void message.error(`加载失败：${(e as Error).message}`);
    } finally {
      setLoading(false);
    }
  }, [search]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const handleView = async (cap: CapabilityItem) => {
    setViewLoading(true);
    setViewingCapability(null);
    setViewOpen(true);
    try {
      const detail = await getCapabilityDetail(cap.name);
      setViewingCapability(detail);
    } catch (e) {
      void message.error(`加载详情失败：${(e as Error).message}`);
    } finally {
      setViewLoading(false);
    }
  };

  const columns: ColumnsType<CapabilityItem> = [
    {
      title: '名称',
      dataIndex: 'name',
      width: 200,
      render: (name: string) => <Tag color="blue">{name}</Tag>,
    },
    { title: '描述', dataIndex: 'description' },
    {
      title: '危险性',
      dataIndex: 'is_dangerous',
      width: 100,
      render: (v: boolean) =>
        v ? <Tag color="red">危险</Tag> : <Tag color="green">安全</Tag>,
    },
    {
      title: '操作',
      key: 'actions',
      width: 100,
      render: (_, r) => (
        <Button size="small" onClick={() => void handleView(r)}>
          查看
        </Button>
      ),
    },
  ];

  return (
    <div>
      <Space style={{ marginBottom: 16 }}>
        <Input.Search
          placeholder="搜索 capability 名称或描述"
          allowClear
          onSearch={setSearch}
          style={{ width: 320 }}
          aria-label="搜索 capability"
        />
      </Space>
      <Table<CapabilityItem>
        rowKey="name"
        columns={columns}
        dataSource={items}
        loading={loading}
        pagination={false}
      />
      <Drawer
        title={viewingCapability ? `Capability「${viewingCapability.name}」` : '查看 Capability'}
        open={viewOpen}
        width={600}
        onClose={() => {
          setViewOpen(false);
          setViewingCapability(null);
        }}
      >
        {viewLoading && <p>加载中…</p>}
        {viewingCapability && (
          <Space direction="vertical" style={{ width: '100%' }} size="large">
            <div>
              <strong>名称：</strong>
              <Tag color="blue">{viewingCapability.name}</Tag>
            </div>
            <div>
              <strong>描述：</strong>
              <p style={{ marginTop: 4 }}>{viewingCapability.description}</p>
            </div>
            <div>
              <strong>危险性：</strong>
              {viewingCapability.is_dangerous ? (
                <Tag color="red">危险</Tag>
              ) : (
                <Tag color="green">安全</Tag>
              )}
            </div>
            {viewingCapability.allowed_plugins !== undefined && (
              <div>
                <strong>允许使用的插件：</strong>
                <p style={{ marginTop: 4 }}>
                  {viewingCapability.allowed_plugins.length > 0
                    ? viewingCapability.allowed_plugins.join(', ')
                    : '无'}
                </p>
              </div>
            )}
          </Space>
        )}
      </Drawer>
    </div>
  );
}
