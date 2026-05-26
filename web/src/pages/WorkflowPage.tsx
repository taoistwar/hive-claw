// WorkflowPage — 列表 + 选中后展示 DagEditor (T115)

import { useCallback, useEffect, useState } from 'react';
import { Button, Drawer, Form, Input, InputNumber, Modal, Space, Table, message } from 'antd';
import type { ColumnsType } from 'antd/es/table';

import { DagEditor } from '../components/DagEditor/DagEditor';
import {
  createWorkflow,
  deleteWorkflow,
  listWorkflows,
  type WorkflowMeta,
} from '../services/workflow';

export default function WorkflowPage() {
  const [items, setItems] = useState<WorkflowMeta[]>([]);
  const [loading, setLoading] = useState(false);
  const [selected, setSelected] = useState<WorkflowMeta | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [form] = Form.useForm();

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const list = await listWorkflows();
      setItems(list.items);
    } catch (e) {
      void message.error(`加载失败：${(e as Error).message}`);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const onCreate = async (values: {
    identifier: string;
    name: string;
    description?: string;
    timeout_ms?: number;
  }) => {
    try {
      const wf = await createWorkflow(values);
      void message.success(`已创建 workflow #${wf.id}`);
      setCreateOpen(false);
      form.resetFields();
      await refresh();
    } catch (e) {
      void message.error(`创建失败：${(e as Error).message}`);
    }
  };

  const onDelete = (wf: WorkflowMeta) => {
    Modal.confirm({
      title: `删除 workflow「${wf.identifier}」？`,
      content: '若被 Tool (kind=2) 引用，将返回 4093 阻止',
      okButtonProps: { danger: true },
      onOk: async () => {
        try {
          await deleteWorkflow(wf.id);
          void message.success('已删除');
          if (selected?.id === wf.id) setSelected(null);
          await refresh();
        } catch (e: unknown) {
          const err = e as { response?: { data?: { code?: number; message?: string } } };
          void message.error(err.response?.data?.message ?? (e as Error).message);
        }
      },
    });
  };

  const columns: ColumnsType<WorkflowMeta> = [
    { title: 'ID', dataIndex: 'id', width: 60 },
    { title: 'identifier', dataIndex: 'identifier' },
    { title: 'name', dataIndex: 'name' },
    { title: 'timeout_ms', dataIndex: 'timeout_ms', width: 110 },
    {
      title: 'actions',
      key: 'actions',
      render: (_, r) => (
        <Space>
          <Button size="small" onClick={() => setSelected(r)}>
            编辑 DAG
          </Button>
          <Button size="small" danger onClick={() => onDelete(r)}>
            删除
          </Button>
        </Space>
      ),
    },
  ];

  return (
    <div>
      <Space style={{ marginBottom: 16 }}>
        <Button type="primary" onClick={() => setCreateOpen(true)}>
          新建 Workflow
        </Button>
      </Space>
      <Table<WorkflowMeta>
        rowKey="id"
        columns={columns}
        dataSource={items}
        loading={loading}
        pagination={false}
      />

      <Drawer
        title={selected ? `DAG 编辑器 — ${selected.identifier}` : ''}
        open={!!selected}
        width="80%"
        onClose={() => setSelected(null)}
        destroyOnClose
      >
        {selected ? (
          <DagEditor workflowId={selected.id} onSaved={() => void refresh()} />
        ) : null}
      </Drawer>

      <Modal
        title="新建 Workflow"
        open={createOpen}
        onCancel={() => setCreateOpen(false)}
        onOk={() => form.submit()}
        destroyOnClose
      >
        <Form form={form} layout="vertical" onFinish={onCreate}>
          <Form.Item name="identifier" label="identifier" rules={[{ required: true }]}>
            <Input placeholder="ingest" />
          </Form.Item>
          <Form.Item name="name" label="name" rules={[{ required: true }]}>
            <Input placeholder="Ingest pipeline" />
          </Form.Item>
          <Form.Item name="description" label="description">
            <Input.TextArea rows={2} />
          </Form.Item>
          <Form.Item name="timeout_ms" label="timeout_ms (默认 30000)">
            <InputNumber min={1000} max={300000} style={{ width: '100%' }} />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
