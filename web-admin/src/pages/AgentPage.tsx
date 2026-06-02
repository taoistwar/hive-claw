// AgentPage — Agent 树 + 详情 + 完整创建/编辑 (T125 + T124)

import { useCallback, useEffect, useState } from 'react';
import { Button, Drawer, Modal, Space, Tabs, Tag, Typography, message } from 'antd';

import { AgentEditor, type AgentFormPayload } from '../components/AgentEditor';
import AgentHookEditor from '../components/AgentHookEditor/AgentHookEditor';
import HookExecutionLog from '../components/AgentHookEditor/HookExecutionLog';
import { AgentTree } from '../components/AgentTree';
import { useAuth } from '../hooks/useAuth';
import {
  createAgent,
  deleteAgent,
  getAgent,
  listAgentTree,
  updateAgent,
  type AgentDetail,
  type AgentTreeNode,
} from '../services/agent';

const { Title, Paragraph, Text } = Typography;

export default function AgentPage() {
  const { admin } = useAuth();
  const [tree, setTree] = useState<AgentTreeNode[]>([]);
  const [selected, setSelected] = useState<AgentDetail | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [editing, setEditing] = useState<AgentDetail | null>(null);
  const [expandedKeys, setExpandedKeys] = useState<number[]>();

  const collectAllKeys = useCallback((items: AgentTreeNode[]): number[] => {
    const keys: number[] = [];
    for (const n of items) {
      keys.push(n.id);
      if (n.children?.length) {
        keys.push(...collectAllKeys(n.children));
      }
    }
    return keys;
  }, []);

  const refresh = useCallback(async () => {
    try {
      const data = await listAgentTree();
      setTree(data);
      setExpandedKeys(collectAllKeys(data));
    } catch (e) {
      void message.error(`加载失败：${(e as Error).message}`);
    }
  }, [collectAllKeys]);

  const expandAll = () => setExpandedKeys(collectAllKeys(tree));

  const collapseAll = () => setExpandedKeys([]);

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

  const onSubmit = async (payload: AgentFormPayload, id?: number) => {
    try {
      if (id) {
        await updateAgent(id, {
          name: payload.name,
          description: payload.description,
          system_prompt: payload.system_prompt,
          parent_agent_id: payload.parent_agent_id,
          model_preset: payload.model_preset ?? undefined,
          tool_ids: payload.tool_ids,
          skill_ids: payload.skill_ids,
          permissions: payload.permissions,
          updated_at: payload.updated_at ?? new Date().toISOString(),
        });
        void message.success('已保存');
        setEditing(null);
        if (selected?.id === id) setSelected(await getAgent(id));
      } else {
        await createAgent({
          identifier: payload.identifier,
          name: payload.name,
          description: payload.description,
          system_prompt: payload.system_prompt,
          parent_agent_id: payload.parent_agent_id,
          model_preset: payload.model_preset ?? undefined,
          tool_ids: payload.tool_ids,
          skill_ids: payload.skill_ids,
          permissions: payload.permissions,
        });
        void message.success('已创建');
        setCreateOpen(false);
      }
      await refresh();
    } catch (e: unknown) {
      const err = e as { response?: { data?: { code?: number; message?: string } } };
      const msg = err.response?.data?.message ?? (e as Error).message;
      void message.error(`操作失败：${msg}`);
    }
  };

  const onDeleteAgent = (agent: AgentDetail) => {
    Modal.confirm({
      title: `删除 Agent「${agent.identifier}」？`,
      content:
        agent.identifier === 'main'
          ? 'main agent 不可删除，将返回 5001'
          : '若有子 Agent 将返回 4093 阻止',
      okButtonProps: { danger: true },
      onOk: async () => {
        try {
          await deleteAgent(agent.id);
          void message.success('已删除');
          if (selected?.id === agent.id) setSelected(null);
          await refresh();
        } catch (e: unknown) {
          const err = e as { response?: { data?: { code?: number; message?: string } } };
          void message.error(err.response?.data?.message ?? (e as Error).message);
        }
      },
    });
  };

  return (
    <div style={{ display: 'flex', gap: 24 }}>
      <div style={{ flex: '0 0 360px' }}>
        <Space style={{ marginBottom: 12 }}>
          <Button type="primary" onClick={() => setCreateOpen(true)}>
            新建 Agent
          </Button>
          <Button onClick={() => void refresh()}>刷新</Button>
          <Button onClick={expandAll}>全部展开</Button>
          <Button onClick={collapseAll}>全部关闭</Button>
        </Space>
        <AgentTree data={tree} onSelect={onSelect} expandedKeys={expandedKeys} onExpand={setExpandedKeys} />
      </div>
      <div style={{ flex: 1 }}>
        {selected ? (
          <>
            <Space style={{ float: 'right' }}>
              <Button onClick={() => setEditing(selected)}>编辑</Button>
              <Button danger onClick={() => onDeleteAgent(selected)}>
                删除
              </Button>
            </Space>
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
                maxHeight: 240,
                overflow: 'auto',
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
        width={680}
        onClose={() => setCreateOpen(false)}
        destroyOnHidden
      >
        <AgentEditor
          currentRole={admin?.role ?? 0}
          onSubmit={onSubmit}
          onCancel={() => setCreateOpen(false)}
        />
      </Drawer>

      <Drawer
        title={editing ? `编辑 Agent — ${editing.identifier}` : ''}
        open={!!editing}
        width={720}
        onClose={() => setEditing(null)}
        destroyOnHidden
      >
        {editing && (
          <Tabs
            defaultActiveKey="basic"
            items={[
              {
                key: 'basic',
                label: '基本信息',
                children: (
                  <AgentEditor
                    initial={editing}
                    currentRole={admin?.role ?? 0}
                    onSubmit={onSubmit}
                    onCancel={() => setEditing(null)}
                  />
                ),
              },
              {
                key: 'hooks',
                label: 'Hook 配置',
                children: (
                  <AgentHookEditor
                    agentId={editing.id}
                    isMainAgent={editing.identifier === 'main'}
                    currentRole={admin?.role ?? 0}
                  />
                ),
              },
              {
                key: 'executions',
                label: '执行历史',
                children: <HookExecutionLog agentId={editing.id} />,
              },
            ]}
          />
        )}
      </Drawer>
    </div>
  );
}
