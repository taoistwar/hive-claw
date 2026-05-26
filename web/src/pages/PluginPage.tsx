// PluginPage — 列表 + 抽屉编辑 + 引用阻塞确认 (T077)

import { useState } from 'react';
import { Button, Drawer, Modal, Space, Table, Tag, message } from 'antd';
import type { ColumnsType } from 'antd/es/table';
import { PluginFilters } from '../components/PluginFilters';
import { PluginUploader } from '../components/PluginUploader';
import { usePlugins } from '../hooks/usePlugins';
import { deletePlugin, type Plugin } from '../services/plugin';

export default function PluginPage() {
  const { items, total, loading, params, setParams, refresh } = usePlugins();
  const [uploadOpen, setUploadOpen] = useState(false);

  const onDelete = (p: Plugin) => {
    Modal.confirm({
      title: `软删除 Plugin「${p.identifier}@${p.version}」？`,
      content: '若被 Function 引用，将返回错误码 4093 阻止删除。',
      okText: '确认删除',
      cancelText: '取消',
      onOk: async () => {
        try {
          await deletePlugin(p.id);
          void message.success('已软删除');
          await refresh();
        } catch (e: unknown) {
          const err = e as { response?: { data?: { code?: number; message?: string } } };
          const code = err.response?.data?.code;
          if (code === 4093) {
            void message.error(err.response?.data?.message ?? '被引用，无法删除');
          } else {
            void message.error(`删除失败：${(e as Error).message}`);
          }
        }
      },
    });
  };

  const columns: ColumnsType<Plugin> = [
    { title: 'ID', dataIndex: 'id', width: 60 },
    { title: 'identifier', dataIndex: 'identifier' },
    { title: 'name', dataIndex: 'name' },
    { title: 'version', dataIndex: 'version', width: 100 },
    {
      title: 'tags',
      dataIndex: 'tags',
      render: (tags?: { id: number; name: string }[]) =>
        tags?.map((t) => <Tag key={t.id}>{t.name}</Tag>) ?? null,
    },
    {
      title: 'size',
      dataIndex: 'size_bytes',
      width: 100,
      render: (n: number) => `${(n / 1024).toFixed(1)} KB`,
    },
    {
      title: 'deleted',
      dataIndex: 'deleted_at',
      width: 90,
      render: (v: string | null) => (v ? <Tag color="red">deleted</Tag> : null),
    },
    {
      title: 'actions',
      key: 'actions',
      render: (_, p) => (
        <Space>
          <Button danger size="small" onClick={() => onDelete(p)} disabled={!!p.deleted_at}>
            删除
          </Button>
        </Space>
      ),
    },
  ];

  return (
    <div>
      <Space style={{ marginBottom: 16 }}>
        <Button type="primary" onClick={() => setUploadOpen(true)}>
          上传 Plugin
        </Button>
      </Space>
      <PluginFilters value={params} onChange={setParams} />
      <Table<Plugin>
        rowKey="id"
        columns={columns}
        dataSource={items}
        loading={loading}
        pagination={{
          current: Math.floor((params.offset ?? 0) / (params.limit ?? 20)) + 1,
          pageSize: params.limit ?? 20,
          total,
          onChange: (page, pageSize) => {
            setParams({ ...params, offset: (page - 1) * pageSize, limit: pageSize });
          },
        }}
        style={{ marginTop: 16 }}
      />
      <Drawer
        title="上传 Plugin"
        open={uploadOpen}
        width={520}
        onClose={() => setUploadOpen(false)}
      >
        <PluginUploader
          onUploaded={() => {
            setUploadOpen(false);
            void refresh();
          }}
        />
      </Drawer>
    </div>
  );
}
