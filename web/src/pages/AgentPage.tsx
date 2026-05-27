// AgentPage — Agent 树 + 简化创建 (T125)
//
// MVP 范围：列树 + 查看详情 + 新建（含 ModelPresetSelect + CapabilityPicker）。
// 完整编辑（含 Monaco system_prompt 编辑 + tool/skill 多选）留待 US6 接通后扩展。

import { useCallback, useEffect, useState } from 'react';
import {
  Button,
  Drawer,
  Form,
  Input,
  InputNumber,
  Space,
  Tag,
  Typography,
  message,
} from 'antd';

import { AgentTree } from '../components/AgentTree';
import { CapabilityPicker } from '../components/CapabilityPicker';
import { ModelPresetSelect } from '../components/ModelPresetSelect';
import { SystemPromptEditor } from '../components/SystemPromptEditor';
import { useAuth } from '../hooks/useAuth';
import {
  createAgent,
  getAgent,
  listAgentTree,
  type AgentDetail,
  type AgentTreeNode,
  type CreateAgent,
} from '../services/agent';

const { Title, Paragraph, Text } = Typography;

export default function AgentPage() {
  const { user } = useAuth();
  const [tree, setTree] = useState<AgentTreeNode[]>([]);
  const [selected, setSelected] = useState<AgentDetail | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [form] = Form.useForm<CreateAgent>();
  const [perms, setPerms] = useState<string[]>([]);
  const [preset, setPreset] = useState<string | null>(null);
  const [systemPrompt, setSystemPrompt] = useState<string>('');

  const refresh = useCallback(async () => {
    try {
      setTree(await listAgentTree());
    } catch (e) {
      void message.error(`加载失败：${(e as Error).message}`);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const onSelect = async (id: number | null) => {
    if (id === null) {
      setSelected(null);
      return;
    }
    try {
      setSelected(await getAgent(id));
    } catch (e) {
      void message.error(`详情加载失败：${(e as Error).message}`);
    }
  };

  const onCreate = async (values: CreateAgent) => {
    try {
      await createAgent({
        ...values,
        system_prompt: systemPrompt,
        permissions: perms,
        model_preset: preset ?? undefined,
      });
      void message.success('创建成功');
      form.resetFields();
      setPerms([]);
      setPreset(null);
      setSystemPrompt('');
      setCreateOpen(false);
      await refresh();
    } catch (e: unknown) {
      const err = e as { response?: { data?: { code?: number; message?: string } } };
      const msg = err.response?.data?.message ?? (e as Error).message;
      void message.error(`创建失败：${msg}`);
    }
  };

  return (
    <div style={{ display: 'flex', gap: 24 }}>
      <div style={{ flex: '0 0 360px' }}>
        <Space style={{ marginBottom: 12 }}>
          <Button type="primary" onClick={() => setCreateOpen(true)}>
            新建 Agent
          </Button>
          <Button onClick={() => void refresh()}>刷新</Button>
        </Space>
        <AgentTree data={tree} onSelect={onSelect} />
      </div>
      <div style={{ flex: 1 }}>
        {selected ? (
          <>
            <Title level={4}>
              {selected.name}{' '}
              <Text type="secondary" style={{ fontSize: 14 }}>
                {selected.identifier}
              </Text>{' '}
              <Tag>depth={selected.depth}</Tag>
              {selected.model_preset ? (
                <Tag color="blue">{selected.model_preset}</Tag>
              ) : (
                <Tag>默认 preset</Tag>
              )}
            </Title>
            <Paragraph type="secondary">{selected.description ?? '—'}</Paragraph>
            <Title level={5}>System Prompt</Title>
            <pre
              style={{
                background: '#fafafa',
                padding: 12,
                whiteSpace: 'pre-wrap',
                fontSize: 13,
              }}
            >
              {selected.system_prompt}
            </pre>
            <Title level={5}>Tools ({selected.tools.length})</Title>
            <Space wrap>
              {selected.tools.map((t) => (
                <Tag key={t.id}>{t.name}</Tag>
              ))}
            </Space>
            <Title level={5} style={{ marginTop: 16 }}>
              Skills ({selected.skills.length})
            </Title>
            <Space wrap>
              {selected.skills.map((s) => (
                <Tag key={s.id}>{s.name}</Tag>
              ))}
            </Space>
            <Title level={5} style={{ marginTop: 16 }}>
              Permissions ({selected.permissions.length})
            </Title>
            <Space wrap>
              {selected.permissions.map((p) => (
                <Tag key={p}>{p}</Tag>
              ))}
            </Space>
          </>
        ) : (
          <Text type="secondary">选择左侧 Agent 查看详情</Text>
        )}
      </div>

      <Drawer
        title="新建 Agent"
        open={createOpen}
        width={520}
        onClose={() => setCreateOpen(false)}
      >
        <Form form={form} layout="vertical" onFinish={onCreate}>
          <Form.Item name="identifier" label="identifier" rules={[{ required: true }]}>
            <Input placeholder="rust-expert" />
          </Form.Item>
          <Form.Item name="name" label="name" rules={[{ required: true }]}>
            <Input placeholder="Rust 专家" />
          </Form.Item>
          <Form.Item name="description" label="description">
            <Input />
          </Form.Item>
          <Form.Item label="system_prompt" required>
            <SystemPromptEditor
              value={systemPrompt}
              onChange={setSystemPrompt}
              height="240px"
            />
          </Form.Item>
          <Form.Item name="parent_agent_id" label="parent_agent_id (留空 = 顶级)">
            <InputNumber min={1} style={{ width: '100%' }} />
          </Form.Item>
          <Form.Item label="model_preset">
            <ModelPresetSelect value={preset} onChange={setPreset} />
          </Form.Item>
          <Form.Item label="permissions">
            <CapabilityPicker
              value={perms}
              onChange={setPerms}
              currentRole={user?.role ?? 0}
            />
          </Form.Item>
          <Form.Item>
            <Button type="primary" htmlType="submit">
              创建
            </Button>
          </Form.Item>
        </Form>
      </Drawer>
    </div>
  );
}
