// AgentEditor — full edit form (T124) used in both create + edit drawers
//
// 输入：可选 initial（编辑模式则不为空）。
// 输出：onSubmit 接收完整 payload；create vs update 由调用方根据有无 id 决定走哪个 API。

import { useEffect, useState } from 'react';
import {
  Button,
  Form,
  Input,
  InputNumber,
  Select,
  Space,
  Tag,
  Typography,
  message,
} from 'antd';

import { CapabilityPicker } from './CapabilityPicker';
import { ModelPresetSelect } from './ModelPresetSelect';
import { SystemPromptEditor } from './SystemPromptEditor';
import { listTools, type ToolItem } from '../services/tool';
import { listSkills, type SkillItem } from '../services/skill';
import type { AgentDetail } from '../services/agent';

const { Text } = Typography;

export interface AgentFormPayload {
  identifier: string;
  name: string;
  description?: string;
  system_prompt: string;
  parent_agent_id?: number;
  model_preset?: string | null;
  tool_ids: number[];
  skill_ids: number[];
  permissions: string[];
  /** 编辑模式需要携带 updated_at 走乐观锁；新建留空 */
  updated_at?: string;
}

export interface AgentEditorProps {
  /** 编辑模式时填入；新建模式留空 */
  initial?: AgentDetail | null;
  currentRole: number;
  onSubmit: (payload: AgentFormPayload, id?: number) => Promise<void>;
  onCancel: () => void;
}

export function AgentEditor({ initial, currentRole, onSubmit, onCancel }: AgentEditorProps) {
  const isEdit = !!initial;
  const [form] = Form.useForm();
  const [systemPrompt, setSystemPrompt] = useState<string>(initial?.system_prompt ?? '');
  const [perms, setPerms] = useState<string[]>(initial?.permissions ?? []);
  const [preset, setPreset] = useState<string | null>(initial?.model_preset ?? null);
  const [toolIds, setToolIds] = useState<number[]>(initial?.tools.map((t) => t.id) ?? []);
  const [skillIds, setSkillIds] = useState<number[]>(initial?.skills.map((s) => s.id) ?? []);
  const [tools, setTools] = useState<ToolItem[]>([]);
  const [skills, setSkills] = useState<SkillItem[]>([]);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void (async () => {
      try {
        const [tl, sl] = await Promise.all([listTools({ limit: 100 }), listSkills({ limit: 100 })]);
        setTools(tl.items);
        setSkills(sl.items);
      } catch (e) {
        void message.error(`资源加载失败：${(e as Error).message}`);
      }
    })();
  }, []);

  useEffect(() => {
    if (initial) {
      form.setFieldsValue({
        identifier: initial.identifier,
        name: initial.name,
        description: initial.description ?? '',
        parent_agent_id: initial.parent_agent_id ?? undefined,
      });
    }
  }, [initial, form]);

  const onFinish = async (values: {
    identifier: string;
    name: string;
    description?: string;
    parent_agent_id?: number;
  }) => {
    if (!systemPrompt.trim()) {
      void message.error('system_prompt 不能为空');
      return;
    }
    setBusy(true);
    try {
      const payload: AgentFormPayload = {
        identifier: values.identifier,
        name: values.name,
        description: values.description || undefined,
        system_prompt: systemPrompt,
        parent_agent_id: values.parent_agent_id,
        model_preset: preset,
        tool_ids: toolIds,
        skill_ids: skillIds,
        permissions: perms,
        updated_at: initial?.updated_at,
      };
      await onSubmit(payload, initial?.id);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Form form={form} layout="vertical" onFinish={onFinish}>
      <Form.Item name="identifier" label="identifier" rules={[{ required: true }]}>
        <Input placeholder="rust-expert" disabled={isEdit} />
      </Form.Item>
      <Form.Item name="name" label="name" rules={[{ required: true }]}>
        <Input placeholder="Rust 专家" />
      </Form.Item>
      <Form.Item name="description" label="description">
        <Input.TextArea rows={4} />
      </Form.Item>
      <Form.Item label="system_prompt" required>
        <SystemPromptEditor value={systemPrompt} onChange={setSystemPrompt} height="280px" />
      </Form.Item>
      {!isEdit && (
        <Form.Item name="parent_agent_id" label="parent_agent_id (留空 = 顶级)">
          <InputNumber min={1} style={{ width: '100%' }} />
        </Form.Item>
      )}
      <Form.Item label="model_preset">
        <ModelPresetSelect value={preset} onChange={setPreset} />
      </Form.Item>
      <Form.Item label={`tools (${toolIds.length})`}>
        <Select
          mode="multiple"
          value={toolIds}
          onChange={setToolIds}
          placeholder="选择已注册 Tool"
          showSearch
          optionFilterProp="label"
          options={tools.map((t) => ({
            value: t.id,
            label: `${t.name} (${t.identifier})`,
          }))}
          aria-label="选择 tool"
        />
      </Form.Item>
      <Form.Item label={`skills (${skillIds.length})`}>
        <Select
          mode="multiple"
          value={skillIds}
          onChange={setSkillIds}
          placeholder="选择已注册 Skill"
          showSearch
          optionFilterProp="label"
          options={skills.map((s) => ({
            value: s.id,
            label: (
              <Space>
                <span>{s.name}</span>
                {s.source === 'builtin' ? <Tag color="purple">builtin</Tag> : null}
              </Space>
            ),
          }))}
          aria-label="选择 skill"
        />
      </Form.Item>
      <Form.Item label="permissions">
        <CapabilityPicker value={perms} onChange={setPerms} currentRole={currentRole} />
      </Form.Item>
      {isEdit && (
        <Text type="secondary" style={{ display: 'block', marginBottom: 12 }}>
          编辑模式自动携带 updated_at={initial?.updated_at} 走乐观锁；冲突会返 4094
        </Text>
      )}
      <Form.Item>
        <Space>
          <Button type="primary" htmlType="submit" loading={busy}>
            {isEdit ? '保存' : '创建'}
          </Button>
          <Button onClick={onCancel}>取消</Button>
        </Space>
      </Form.Item>
    </Form>
  );
}
