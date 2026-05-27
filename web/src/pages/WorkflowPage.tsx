// WorkflowPage — 列表 + 编辑基本信息 + 选中后展示 DagEditor (T115)

import { useCallback, useEffect, useState } from 'react';
import { Button, Drawer, Form, Input, InputNumber, Modal, Space, Table, Tooltip, message } from 'antd';
import type { ColumnsType } from 'antd/es/table';
import dayjs from 'dayjs';

import { DagEditor } from '../components/DagEditor/DagEditor';
import {
  createWorkflow,
  deleteWorkflow,
  listWorkflows,
  updateWorkflow,
  type WorkflowMeta,
} from '../services/workflow';

function formatDate(v: string | undefined): string {
  if (!v) return '—';
  return dayjs(v).format('YYYY-MM-DD HH:mm');
}

function truncateDescription(v: string | null | undefined): string {
  if (!v) return '—';
  if (v.length <= 40) return v;
  return `${v.slice(0, 40)}…`;
}

export default function WorkflowPage() {
  const [items, setItems] = useState<WorkflowMeta[]>([]);
  const [loading, setLoading] = useState(false);
  const [selected, setSelected] = useState<WorkflowMeta | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [createForm] = Form.useForm();
  const [editOpen, setEditOpen] = useState(false);
  const [editForm] = Form.useForm();
  const [editingWf, setEditingWf] = useState<WorkflowMeta | null>(null);

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
      createForm.resetFields();
      await refresh();
    } catch (e) {
      void message.error(`创建失败：${(e as Error).message}`);
    }
  };

  const openEdit = (wf: WorkflowMeta) => {
    setEditingWf(wf);
    editForm.setFieldsValue({
      name: wf.name,
      description: wf.description,
      timeout_ms: wf.timeout_ms,
    });
    setEditOpen(true);
  };

  const onEdit = async (values: {
    name: string;
    description?: string;
    timeout_ms?: number;
  }) => {
    if (!editingWf) return;
    try {
      await updateWorkflow(editingWf.id, {
        ...values,
        updated_at: editingWf.updated_at,
      });
      void message.success('更新成功');
      editForm.resetFields();
      setEditOpen(false);
      setEditingWf(null);
      await refresh();
    } catch (e) {
      void message.error(`更新失败：${(e as Error).message}`);
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
    {
      title: 'description',
      dataIndex: 'description',
      ellipsis: true,
      width: 160,
      render: (v: string | null) => (
        <Tooltip title={v || undefined}>
          {truncateDescription(v)}
        </Tooltip>
      ),
    },
    { title: 'timeout_ms', dataIndex: 'timeout_ms', width: 110 },
    {
      title: 'created_at',
      dataIndex: 'created_at',
      width: 140,
      render: (v: string) => formatDate(v),
    },
    {
      title: 'updated_at',
      dataIndex: 'updated_at',
      width: 140,
      render: (v: string) => formatDate(v),
    },
    {
      title: 'actions',
      key: 'actions',
      width: 220,
      render: (_, r) => (
        <Space>
          <Button size="small" onClick={() => setSelected(r)}>
            编辑 DAG
          </Button>
          <Button size="small" onClick={() => openEdit(r)}>
            编辑信息
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
        onOk={() => createForm.submit()}
        destroyOnClose
      >
        <Form form={createForm} layout="vertical" onFinish={onCreate}>
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

      <Modal
        title="编辑 Workflow 基本信息"
        open={editOpen}
        onCancel={() => {
          setEditOpen(false);
          setEditingWf(null);
        }}
        onOk={() => editForm.submit()}
        destroyOnClose
      >
        <Form form={editForm} layout="vertical" onFinish={onEdit}>
          <Form.Item name="name" label="name" rules={[{ required: true }]}>
            <Input />
          </Form.Item>
          <Form.Item name="description" label="description">
            <Input.TextArea rows={3} />
          </Form.Item>
          <Form.Item name="timeout_ms" label="timeout_ms">
            <InputNumber min={1000} max={300000} style={{ width: '100%' }} />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
