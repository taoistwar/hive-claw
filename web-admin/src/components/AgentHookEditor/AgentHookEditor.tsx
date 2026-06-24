import React, { useState, useEffect } from 'react';
import { Table, Tag, Switch, Button, Popconfirm, Space, message } from 'antd';
import { PlusOutlined, EditOutlined, DeleteOutlined } from '@ant-design/icons';
import type { ColumnsType } from 'antd/es/table';
import {
  listHooks,
  updateHook,
  deleteHook,
  type AgentHook,
  TRIGGER_POINT_LABELS,
  ACTION_TYPE_LABELS,
  type TriggerPoint,
  type ActionType,
} from '../../services/agentHook';
import HookFormModal from './HookFormModal';

interface Props {
  agentId: number;
  isMainAgent: boolean;
  currentRole?: number;
}

const AgentHookEditor: React.FC<Props> = ({ agentId, isMainAgent, currentRole }) => {
  const [hooks, setHooks] = useState<AgentHook[]>([]);
  const [loading, setLoading] = useState(false);
  const [modalOpen, setModalOpen] = useState(false);
  const [editingHook, setEditingHook] = useState<AgentHook | null>(null);

  const loadHooks = async () => {
    setLoading(true);
    try {
      const data = await listHooks(agentId);
      setHooks(data);
    } catch {
      message.error('加载 Hook 列表失败');
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    loadHooks();
  }, [agentId]);

  const handleToggle = async (hook: AgentHook, enabled: boolean) => {
    try {
      await updateHook(agentId, hook.id, { enabled, updated_at: hook.updated_at });
      message.success(enabled ? 'Hook 已启用' : 'Hook 已禁用');
      loadHooks();
    } catch {
      message.error('操作失败');
    }
  };

  const handleDelete = async (hookId: number) => {
    try {
      await deleteHook(agentId, hookId);
      message.success('Hook 已删除');
      loadHooks();
    } catch {
      message.error('删除失败');
    }
  };

  const handleSave = async (_values?: Record<string, unknown>) => {
    setModalOpen(false);
    setEditingHook(null);
    loadHooks();
  };

  const columns: ColumnsType<AgentHook> = [
    { title: '名称', dataIndex: 'name', key: 'name', width: 150 },
    {
      title: '触发点',
      dataIndex: 'trigger_point',
      key: 'trigger_point',
      width: 140,
      render: (v: TriggerPoint) => TRIGGER_POINT_LABELS[v] ?? v,
    },
    {
      title: '动作类型',
      dataIndex: 'action_type',
      key: 'action_type',
      width: 130,
      render: (v: ActionType) => <Tag>{ACTION_TYPE_LABELS[v] ?? v}</Tag>,
    },
    {
      title: '阻塞模式',
      dataIndex: 'blocking_mode',
      key: 'blocking_mode',
      width: 90,
      render: (v: boolean) => (v ? <Tag color="red">阻塞</Tag> : <Tag>非阻塞</Tag>),
    },
    {
      title: '超时(ms)',
      dataIndex: 'timeout_ms',
      key: 'timeout_ms',
      width: 90,
    },
    {
      title: '启用',
      dataIndex: 'enabled',
      key: 'enabled',
      width: 70,
      render: (v: boolean, record: AgentHook) => (
        <Switch
          checked={v}
          size="small"
          onChange={(checked) => handleToggle(record, checked)}
        />
      ),
    },
    {
      title: '操作',
      key: 'actions',
      width: 120,
      render: (_: unknown, record: AgentHook) => (
        <Space>
          <Button
            type="link"
            size="small"
            icon={<EditOutlined />}
            onClick={() => {
              setEditingHook(record);
              setModalOpen(true);
            }}
          />
          <Popconfirm
            title="确定删除此 Hook？"
            onConfirm={() => handleDelete(record.id)}
          >
            <Button type="link" size="small" danger icon={<DeleteOutlined />} />
          </Popconfirm>
        </Space>
      ),
    },
  ];

  if (isMainAgent && currentRole !== 3) {
    return (
      <div style={{ padding: 24, textAlign: 'center', color: '#999' }}>
        入口 Agent「main」的 Hook 配置需 Super 角色操作
      </div>
    );
  }

  return (
    <div>
      <div style={{ marginBottom: 16 }}>
        <Button
          type="primary"
          icon={<PlusOutlined />}
          onClick={() => {
            setEditingHook(null);
            setModalOpen(true);
          }}
        >
          添加 Hook
        </Button>
      </div>
      <Table
        columns={columns}
        dataSource={hooks}
        rowKey="id"
        loading={loading}
        pagination={false}
        size="small"
      />
      <HookFormModal
        open={modalOpen}
        agentId={agentId}
        editingHook={editingHook}
        onClose={() => {
          setModalOpen(false);
          setEditingHook(null);
        }}
        onSave={handleSave}
      />
    </div>
  );
};

export default AgentHookEditor;
