import { Drawer, Form, Input, Select, Space, Tag, Typography, Empty, InputNumber, Divider, message } from 'antd';
import { useState, useEffect, useMemo } from 'react';
import { listFunctions, type FunctionItem } from '../../services/function';
import { listModelPresets, type ModelPreset } from '../../services/agent';
import {
  type AnswerNodeConfig,
  type InputSource,
  type InputSpec,
  RESERVED_INPUT_KEYS,
  USER_INPUT_FIELDS,
  type AgentContextCategory,
} from '../../services/workflow';
import type { Node, Edge } from 'reactflow';

const { Text, Title } = Typography;

const AGENT_CONTEXT_CATEGORIES: { value: AgentContextCategory; label: string }[] = [
  { value: 'user_input', label: '用户输入' },
  { value: 'entities', label: '实体' },
  { value: 'tool_results', label: '工具结果' },
  { value: 'state_changes', label: '状态变更' },
  { value: 'extensions', label: '扩展' },
];

/** 客户端校验：变量名不能空、不能与保留字冲突 */
function validateInputFieldName(name: string): string | null {
  if (!name.trim()) return '变量名不能为空';
  if (RESERVED_INPUT_KEYS.includes(name as typeof RESERVED_INPUT_KEYS[number])) {
    return `变量名 "${name}" 是系统保留字，不能使用`;
  }
  return null;
}

interface InputVar {
  key: string;
  type: string;
  description?: string;
  required: boolean;
}

interface NodeDetailDrawerProps {
  open: boolean;
  onClose: () => void;
  nodeType: 'start' | 'end' | 'function' | 'generate_answer';
  nodeKey: string;
  functionId?: number | null;
  inputSchema?: Record<string, unknown> | null;
  outputSchema?: Record<string, unknown> | null;
  answerConfig?: AnswerNodeConfig | null;
  onUpdateStartNode?: (vars: Record<string, unknown>) => void;
  onUpdateEndNode?: (vars: Record<string, unknown>) => void;
  onUpdateAnswerNode?: (config: AnswerNodeConfig) => void;
  onUpdateFunctionNode?: (nodeKey: string, inputMapping: InputSpec) => void;
  functionInputMapping?: InputSpec | null;
  allNodes?: Node[];
  allEdges?: Edge[];
  allFunctions?: FunctionItem[];
}

function parseInputVars(schema?: Record<string, unknown> | null): InputVar[] {
  if (!schema?.properties) return [];
  const required = (schema.required as string[]) || [];
  const props = schema.properties as Record<string, Record<string, unknown>>;
  return Object.entries(props).map(([key, val]) => ({
    key,
    type: (val.type as string) || 'string',
    description: val.description as string | undefined,
    required: required.includes(key),
  }));
}

function StartNodePanel(props: {
  inputSchema?: Record<string, unknown> | null;
  onUpdate?: (vars: Record<string, unknown>) => void;
}) {
  const [vars, setVars] = useState<InputVar[]>(() => parseInputVars(props.inputSchema));
  const [addVarOpen, setAddVarOpen] = useState(false);
  const [newVarKey, setNewVarKey] = useState('');
  const [newVarType, setNewVarType] = useState('string');
  const [newVarDesc, setNewVarDesc] = useState('');
  const [newVarRequired, setNewVarRequired] = useState(false);
  const [editingVarKey, setEditingVarKey] = useState<string | null>(null);

  const handleAddVar = () => {
    if (!newVarKey) return;
    const newVar: InputVar = {
      key: newVarKey,
      type: newVarType,
      description: newVarDesc || undefined,
      required: newVarRequired,
    };
    const updated = [...vars, newVar];
    setVars(updated);
    setNewVarKey('');
    setNewVarType('string');
    setNewVarDesc('');
    setNewVarRequired(false);
    setAddVarOpen(false);
  };

  const handleRemoveVar = (key: string) => {
    setVars(vars.filter((v) => v.key !== key));
  };

  const handleUpdateVar = (oldKey: string, updated: InputVar) => {
    setVars(vars.map((v) => (v.key === oldKey ? updated : v)));
    setEditingVarKey(null);
  };

  const handleSave = () => {
    const properties: Record<string, unknown> = {};
    const required: string[] = [];
    for (const v of vars) {
      properties[v.key] = {
        type: v.type,
        ...(v.description && { description: v.description }),
      };
      if (v.required) required.push(v.key);
    }
    const schema: Record<string, unknown> = { type: 'object', properties, required };
    props.onUpdate?.(schema);
  };

  return (
    <Space direction="vertical" style={{ width: '100%' }} size="large">
      <div>
        <Title level={5}>起始节点配置</Title>
        <Text type="secondary">
          配置工作流的输入变量，这些变量将作为工作流的 input_schema。
        </Text>
      </div>

      <div>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 8 }}>
          <Text strong>输入变量 ({vars.length})</Text>
          <a onClick={() => setAddVarOpen(true)}>+ 添加变量</a>
        </div>

        {addVarOpen && (
          <div style={{ background: '#fafafa', padding: 12, borderRadius: 8, marginBottom: 12 }}>
            <Space direction="vertical" style={{ width: '100%' }} size="middle">
              <Input
                placeholder="变量名 (英文)"
                value={newVarKey}
                onChange={(e) => setNewVarKey(e.target.value.replace(/[^a-zA-Z0-9_]/g, ''))}
              />
              <Space>
                <Text>类型:</Text>
                <Select
                  value={newVarType}
                  onChange={setNewVarType}
                  style={{ width: 120 }}
                  options={[
                    { label: '字符串', value: 'string' },
                    { label: '数字', value: 'number' },
                    { label: '布尔', value: 'boolean' },
                    { label: '数组', value: 'array' },
                    { label: '对象', value: 'object' },
                  ]}
                />
              </Space>
              <Input
                placeholder="描述 (可选)"
                value={newVarDesc}
                onChange={(e) => setNewVarDesc(e.target.value)}
              />
              <label>
                <input
                  type="checkbox"
                  checked={newVarRequired}
                  onChange={(e) => setNewVarRequired(e.target.checked)}
                  style={{ marginRight: 4 }}
                />
                必填
              </label>
              <Space>
                <a onClick={handleAddVar}>确认添加</a>
                <a onClick={() => setAddVarOpen(false)}>取消</a>
              </Space>
            </Space>
          </div>
        )}

        {vars.length === 0 ? (
          <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="暂无输入变量" style={{ margin: '20px 0' }} />
        ) : (
          <Space direction="vertical" style={{ width: '100%' }} size="middle">
            {vars.map((v) => (
              <div
                key={v.key}
                style={{
                  border: '1px solid #f0f0f0',
                  borderRadius: 8,
                  padding: '8px 12px',
                  background: '#fff',
                  cursor: editingVarKey === v.key ? 'default' : 'pointer',
                }}
                onClick={() => {
                  if (editingVarKey !== v.key) setEditingVarKey(v.key);
                }}
              >
                {editingVarKey === v.key ? (
                  <EditableVarRow
                    var_={v}
                    onSave={handleUpdateVar}
                    onCancel={() => setEditingVarKey(null)}
                  />
                ) : (
                  <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                    <Space>
                      <Text strong>{v.key}</Text>
                      <Tag color="blue">{v.type}</Tag>
                      {v.required && <Tag color="red">必填</Tag>}
                    </Space>
                    <Space size={4}>
                      <a onClick={(e) => { e.stopPropagation(); setEditingVarKey(v.key); }}>
                        编辑
                      </a>
                      <a onClick={(e) => { e.stopPropagation(); handleRemoveVar(v.key); }} style={{ color: '#ff4d4f' }}>
                        删除
                      </a>
                    </Space>
                  </div>
                )}
                {editingVarKey !== v.key && v.description && (
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    {v.description}
                  </Text>
                )}
              </div>
            ))}
          </Space>
        )}
      </div>

      <div style={{ textAlign: 'right' }}>
        <a onClick={handleSave} style={{ fontSize: 14 }}>
          保存配置
        </a>
      </div>
    </Space>
  );
}

function EditableVarRow(props: {
  var_: InputVar;
  onSave: (oldKey: string, updated: InputVar) => void;
  onCancel: () => void;
}) {
  const [type, setType] = useState(props.var_.type);
  const [description, setDescription] = useState(props.var_.description || '');
  const [required, setRequired] = useState(props.var_.required);

  const handleSave = () => {
    props.onSave(props.var_.key, {
      key: props.var_.key,
      type,
      description: description || undefined,
      required,
    });
  };

  return (
    <div onClick={(e) => e.stopPropagation()}>
      <Space direction="vertical" style={{ width: '100%' }} size="small">
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <Text strong>{props.var_.key}</Text>
        </div>
        <Select
          value={type}
          onChange={setType}
          size="small"
          style={{ width: 120 }}
          options={[
            { label: '字符串', value: 'string' },
            { label: '数字', value: 'number' },
            { label: '布尔', value: 'boolean' },
            { label: '数组', value: 'array' },
            { label: '对象', value: 'object' },
          ]}
        />
        <Input
          size="small"
          placeholder="描述 (可选)"
          value={description}
          onChange={(e) => setDescription(e.target.value)}
        />
        <label>
          <input
            type="checkbox"
            checked={required}
            onChange={(e) => setRequired(e.target.checked)}
            style={{ marginRight: 4 }}
          />
          必填
        </label>
        <Space>
          <a onClick={handleSave}>保存</a>
          <a onClick={props.onCancel}>取消</a>
        </Space>
      </Space>
    </div>
  );
}

function EndNodePanel(props: {
  outputSchema?: Record<string, unknown> | null;
  onUpdate?: (vars: Record<string, unknown>) => void;
}) {
  const [vars, setVars] = useState<InputVar[]>(() => parseInputVars(props.outputSchema));
  const [addVarOpen, setAddVarOpen] = useState(false);
  const [newVarKey, setNewVarKey] = useState('');
  const [newVarType, setNewVarType] = useState('string');
  const [newVarDesc, setNewVarDesc] = useState('');
  const [newVarRequired, setNewVarRequired] = useState(false);
  const [editingVarKey, setEditingVarKey] = useState<string | null>(null);

  const handleAddVar = () => {
    if (!newVarKey) return;
    const newVar: InputVar = {
      key: newVarKey,
      type: newVarType,
      description: newVarDesc || undefined,
      required: newVarRequired,
    };
    const updated = [...vars, newVar];
    setVars(updated);
    setNewVarKey('');
    setNewVarType('string');
    setNewVarDesc('');
    setNewVarRequired(false);
    setAddVarOpen(false);
  };

  const handleRemoveVar = (key: string) => {
    setVars(vars.filter((v) => v.key !== key));
  };

  const handleUpdateVar = (oldKey: string, updated: InputVar) => {
    setVars(vars.map((v) => (v.key === oldKey ? updated : v)));
    setEditingVarKey(null);
  };

  const handleSave = () => {
    const properties: Record<string, unknown> = {};
    const required: string[] = [];
    for (const v of vars) {
      properties[v.key] = {
        type: v.type,
        ...(v.description && { description: v.description }),
      };
      if (v.required) required.push(v.key);
    }
    const schema: Record<string, unknown> = { type: 'object', properties, required };
    props.onUpdate?.(schema);
  };

  return (
    <Space direction="vertical" style={{ width: '100%' }} size="large">
      <div>
        <Title level={5}>结束节点配置</Title>
        <Text type="secondary">
          配置工作流的输出变量，这些变量将作为工作流的 output_schema。
        </Text>
      </div>

      <div>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 8 }}>
          <Text strong>输出变量 ({vars.length})</Text>
          <a onClick={() => setAddVarOpen(true)}>+ 添加变量</a>
        </div>

        {addVarOpen && (
          <div style={{ background: '#fafafa', padding: 12, borderRadius: 8, marginBottom: 12 }}>
            <Space direction="vertical" style={{ width: '100%' }} size="middle">
              <Input
                placeholder="变量名 (英文)"
                value={newVarKey}
                onChange={(e) => setNewVarKey(e.target.value.replace(/[^a-zA-Z0-9_]/g, ''))}
              />
              <Space>
                <Text>类型:</Text>
                <Select
                  value={newVarType}
                  onChange={setNewVarType}
                  style={{ width: 120 }}
                  options={[
                    { label: '字符串', value: 'string' },
                    { label: '数字', value: 'number' },
                    { label: '布尔', value: 'boolean' },
                    { label: '数组', value: 'array' },
                    { label: '对象', value: 'object' },
                  ]}
                />
              </Space>
              <Input
                placeholder="描述 (可选)"
                value={newVarDesc}
                onChange={(e) => setNewVarDesc(e.target.value)}
              />
              <label>
                <input
                  type="checkbox"
                  checked={newVarRequired}
                  onChange={(e) => setNewVarRequired(e.target.checked)}
                  style={{ marginRight: 4 }}
                />
                必填
              </label>
              <Space>
                <a onClick={handleAddVar}>确认添加</a>
                <a onClick={() => setAddVarOpen(false)}>取消</a>
              </Space>
            </Space>
          </div>
        )}

        {vars.length === 0 ? (
          <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="暂无输出变量" style={{ margin: '20px 0' }} />
        ) : (
          <Space direction="vertical" style={{ width: '100%' }} size="middle">
            {vars.map((v) => (
              <div
                key={v.key}
                style={{
                  border: '1px solid #f0f0f0',
                  borderRadius: 8,
                  padding: '8px 12px',
                  background: '#fff',
                  cursor: editingVarKey === v.key ? 'default' : 'pointer',
                }}
                onClick={() => {
                  if (editingVarKey !== v.key) setEditingVarKey(v.key);
                }}
              >
                {editingVarKey === v.key ? (
                  <EditableVarRow
                    var_={v}
                    onSave={handleUpdateVar}
                    onCancel={() => setEditingVarKey(null)}
                  />
                ) : (
                  <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                    <Space>
                      <Text strong>{v.key}</Text>
                      <Tag color="blue">{v.type}</Tag>
                      {v.required && <Tag color="red">必填</Tag>}
                    </Space>
                    <Space size={4}>
                      <a onClick={(e) => { e.stopPropagation(); setEditingVarKey(v.key); }}>
                        编辑
                      </a>
                      <a onClick={(e) => { e.stopPropagation(); handleRemoveVar(v.key); }} style={{ color: '#ff4d4f' }}>
                        删除
                      </a>
                    </Space>
                  </div>
                )}
                {editingVarKey !== v.key && v.description && (
                  <Text type="secondary" style={{ fontSize: 12 }}>
                    {v.description}
                  </Text>
                )}
              </div>
            ))}
          </Space>
        )}
      </div>

      <div style={{ textAlign: 'right' }}>
        <a onClick={handleSave} style={{ fontSize: 14 }}>
          保存配置
        </a>
      </div>
    </Space>
  );
}

function AnswerNodePanel(props: {
  config?: AnswerNodeConfig | null;
  nodeKey: string;
  onUpdate?: (config: AnswerNodeConfig) => void;
  upstreamNodes?: Array<{ node_key: string; node_type: string; function_id?: number | null }>;
  nodeOutputFields?: (node_key: string) => { value: string; label: string }[];
}) {
  const [systemPrompt, setSystemPrompt] = useState(props.config?.system_prompt || '');
  const [modelPreset, setModelPreset] = useState<string | undefined>(props.config?.model_preset);
  const [historyWindow, setHistoryWindow] = useState(props.config?.history_window ?? 0);
  // 结构化 input_mapping（与 function_node 共享同一字段）
  const [inputMapping, setInputMapping] = useState<InputSpec>(props.config?.input_mapping ?? {});
  const [presets, setPresets] = useState<ModelPreset[]>([]);
  const [presetsLoaded, setPresetsLoaded] = useState(false);
  const [editingKey, setEditingKey] = useState<string | null>(null);
  const [draftKey, setDraftKey] = useState('');
  const [draftSource, setDraftSource] = useState<InputSource>({ kind: 'upstream', node_key: '', field: '' });
  const [addOpen, setAddOpen] = useState(false);

  const upstreamOptions = (props.upstreamNodes || []).map((n) => ({
    value: n.node_key,
    label: `${n.node_key}${n.node_type === 'start_node' ? ' (开始)' : n.node_type === 'function_node' ? ' (函数)' : n.node_type === 'generate_answer_node' ? ' (回答)' : ''}`,
  }));

  const fieldOptions = (props.nodeOutputFields && draftSource.kind === 'upstream')
    ? props.nodeOutputFields(draftSource.node_key)
    : [];

  useEffect(() => {
    void (async () => {
      try {
        const list = await listModelPresets();
        setPresets(list);
      } finally {
        setPresetsLoaded(true);
      }
    })();
  }, []);

  const resetDraft = () => {
    setDraftKey('');
    setDraftSource({ kind: 'upstream', node_key: '', field: '' });
  };

  const startAdd = () => {
    resetDraft();
    setEditingKey(null);
    setAddOpen(true);
  };

  const startEdit = (key: string) => {
    setEditingKey(key);
    setDraftKey(key);
    setDraftSource(inputMapping[key]);
    setAddOpen(true);
  };

  const cancelDraft = () => {
    setAddOpen(false);
    setEditingKey(null);
    resetDraft();
  };

  const applyDraft = () => {
    const newKey = draftKey.trim();
    const err = validateInputFieldName(newKey);
    if (err) {
      void message.error(err);
      return;
    }
    // 编辑模式：若 key 改了，删除旧 key
    const next: InputSpec = { ...inputMapping };
    if (editingKey && editingKey !== newKey) {
      delete next[editingKey];
    }
    if (next[newKey] && newKey !== editingKey) {
      void message.error(`变量名 "${newKey}" 已存在`);
      return;
    }
    // 清理空字段
    const src: InputSource = draftSource;
    if (src.kind === 'upstream') {
      if (!src.node_key.trim()) {
        void message.error('请选择上游节点');
        return;
      }
      next[newKey] = { kind: 'upstream', node_key: src.node_key, field: src.field || undefined };
    } else if (src.kind === 'custom') {
      next[newKey] = { kind: 'custom', value: src.value };
    } else if (src.kind === 'agent_context') {
      if (!src.key.trim()) {
        void message.error('请输入 AgentContext key');
        return;
      }
      next[newKey] = {
        kind: 'agent_context',
        category: src.category,
        key: src.key,
        sub_key: src.sub_key || undefined,
      };
    }
    setInputMapping(next);
    cancelDraft();
  };

  const removeVar = (key: string) => {
    const next = { ...inputMapping };
    delete next[key];
    setInputMapping(next);
  };

  const handleSave = () => {
    const config: AnswerNodeConfig = {
      system_prompt: systemPrompt,
      model_preset: modelPreset || undefined,
      history_window: historyWindow,
      input_mapping: inputMapping,
    };
    props.onUpdate?.(config);
  };

  const renderDraftSourceForm = (key: string) => {
    return (
      <Space direction="vertical" style={{ width: '100%' }} size="middle">
        <Input
          placeholder="变量名 (英文，如 query)"
          value={draftKey}
          onChange={(e) => setDraftKey(e.target.value.replace(/[^a-zA-Z0-9_]/g, ''))}
        />
        {renderInputSourceForm(draftSource, setDraftSource, upstreamOptions, fieldOptions)}
        <Space>
          <a onClick={applyDraft}>{editingKey === draftKey || !editingKey ? '确认添加' : '保存修改'}</a>
          <a onClick={cancelDraft}>取消</a>
        </Space>
      </Space>
    );
  };

  return (
    <Space direction="vertical" style={{ width: '100%' }} size="middle">
      <div>
        <Title level={5}>生成回答节点配置</Title>
        <Text type="secondary">
          配置 LLM 的系统提示词、模型和变量，生成最终回答。
        </Text>
      </div>

      <Form.Item label="模型选择">
        <Select
          value={modelPreset || undefined}
          onChange={setModelPreset}
          placeholder="选择模型 preset（留空使用默认）"
          allowClear
          loading={!presetsLoaded}
          style={{ width: '100%' }}
          options={presets.map((p) => ({
            value: p.name,
            label: `${p.name}${p.is_default ? ' (默认)' : ''}`,
          }))}
        />
      </Form.Item>

      <Form.Item label="历史消息窗口">
        <InputNumber
          min={0}
          max={50}
          value={historyWindow}
          onChange={(v) => setHistoryWindow(v ?? 0)}
          style={{ width: '100%' }}
        />
        <Text type="secondary" style={{ fontSize: 11 }}>
          传递给 LLM 的历史消息数量
        </Text>
      </Form.Item>

      <Form.Item label="系统提示词">
        <Input.TextArea
          value={systemPrompt}
          onChange={(e) => setSystemPrompt(e.target.value)}
          placeholder={`你是一个智能助手，请根据以下内容回答用户问题：\n{query}\n\n请用简洁的语言回复。`}
          rows={6}
          maxLength={32000}
          showCount
        />
        <Text type="secondary" style={{ fontSize: 11 }}>
          使用 {'{变量名}'} 引用上游节点的输出值
        </Text>
      </Form.Item>

      <Divider />

      <div>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 8 }}>
          <Text strong>输入变量配置 ({Object.keys(inputMapping).length})</Text>
          <a onClick={startAdd}>+ 添加变量</a>
        </div>
        <Text type="secondary" style={{ fontSize: 11, display: 'block', marginBottom: 12 }}>
          变量值在运行时会自动从前置节点 / AgentContext / 字面量获取。变量名与系统提示词中的 {'{变量名}'} 对应。
        </Text>

        {addOpen && (
          <div style={{ background: '#fafafa', padding: 12, borderRadius: 8, marginBottom: 12 }}>
            {renderDraftSourceForm(draftKey)}
          </div>
        )}

        {Object.keys(inputMapping).length === 0 && !addOpen ? (
          <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="暂无变量" style={{ margin: '20px 0' }} />
        ) : (
          <Space direction="vertical" style={{ width: '100%' }} size="small">
            {Object.entries(inputMapping).map(([key, src]) => {
              const isEditing = addOpen && editingKey === key;
              return (
                <div
                  key={key}
                  style={{
                    border: isEditing ? '1px solid #1890ff' : '1px solid #f0f0f0',
                    borderRadius: 8,
                    padding: '8px 12px',
                    background: isEditing ? '#e6f7ff' : '#fff',
                  }}
                >
                  {!isEditing && (
                    <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                      <Space direction="vertical" size={0}>
                        <Space>
                          <Text strong>{key}</Text>
                          <Tag color="cyan">
                            {src.kind === 'upstream' ? '上游' : src.kind === 'custom' ? '字面量' : 'AgentContext'}
                          </Tag>
                        </Space>
                        {src.kind === 'upstream' && (
                          <Text type="secondary" style={{ fontSize: 11 }}>
                            源: {src.node_key}{src.field ? `.${src.field}` : ''}
                          </Text>
                        )}
                        {src.kind === 'custom' && (
                          <Text type="secondary" style={{ fontSize: 11 }}>
                            值: {typeof src.value === 'string' ? src.value : JSON.stringify(src.value)}
                          </Text>
                        )}
                        {src.kind === 'agent_context' && (
                          <Text type="secondary" style={{ fontSize: 11 }}>
                            源: {src.category}.{src.key}{src.sub_key ? `.${src.sub_key}` : ''}
                          </Text>
                        )}
                      </Space>
                      <Space>
                        <a onClick={() => startEdit(key)}>编辑</a>
                        <a onClick={() => removeVar(key)} style={{ color: '#ff4d4f' }}>删除</a>
                      </Space>
                    </div>
                  )}
                </div>
              );
            })}
          </Space>
        )}
      </div>

      <div style={{ textAlign: 'right', marginBottom: 8 }}>
        <a onClick={handleSave} style={{ fontSize: 14 }}>
          保存配置
        </a>
      </div>

      <div
        style={{
          border: '1px solid #d9f7be',
          borderRadius: 8,
          padding: '8px 10px',
          background: '#fcffe6',
        }}
      >
        <Text strong style={{ fontSize: 12, marginBottom: 4, display: 'block' }}>
          输出变量 (2)
        </Text>
        <Space direction="vertical" style={{ width: '100%' }} size={2}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <Text code style={{ fontSize: 12 }}>answer</Text>
            <Text type="secondary" style={{ fontSize: 11 }}>LLM 生成的最终回答文本</Text>
            <Text style={{ fontSize: 11, color: '#b37feb', background: '#f9f0ff', padding: '0 4px', borderRadius: 3 }}>
              样例："您好，根据查询结果…"
            </Text>
          </div>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <Text code style={{ fontSize: 12 }}>model_preset</Text>
            <Text type="secondary" style={{ fontSize: 11 }}>使用的模型预设名称</Text>
            <Text style={{ fontSize: 11, color: '#b37feb', background: '#f9f0ff', padding: '0 4px', borderRadius: 3 }}>
              样例："gpt-4"
            </Text>
          </div>
        </Space>
      </div>
    </Space>
  );
}

/** 将 JSON Schema 解析为字段列表用于展示 */
function parseSchemaToFieldList(schema: unknown): Array<{ name: string; type: string; required: boolean; description?: string; default?: unknown }> {
  if (!schema || typeof schema !== 'object') return [];
  const s = schema as Record<string, unknown>;
  const properties = s.properties as Record<string, Record<string, unknown>> | undefined;
  if (!properties) return [];
  const required: string[] = Array.isArray(s.required) ? s.required as string[] : [];
  return Object.entries(properties).map(([name, prop]) => ({
    name,
    type: (prop.type as string) || 'string',
    required: required.includes(name),
    description: prop.description as string | undefined,
    default: prop.default,
  }));
}

function SchemaFieldList({ title, fields }: { title: string; fields: ReturnType<typeof parseSchemaToFieldList> }) {
  return (
    <div>
      <Text strong style={{ marginBottom: 8, display: 'block' }}>{title} ({fields.length})</Text>
      {fields.length === 0 ? (
        <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="无字段" style={{ margin: '12px 0' }} />
      ) : (
        <Space direction="vertical" style={{ width: '100%' }} size="small">
          {fields.map((f) => (
            <div
              key={f.name}
              style={{
                border: '1px solid #f0f0f0',
                borderRadius: 8,
                padding: '8px 12px',
                background: '#fff',
              }}
            >
              <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                <div style={{ display: 'flex', flexDirection: 'column', gap: 2, flex: 1 }}>
                  <Space size={4}>
                    <Text strong style={{ fontSize: 13 }}>{f.name}</Text>
                    <Tag color="blue" style={{ fontSize: 11 }}>{f.type}</Tag>
                    {f.required && <Tag color="red" style={{ fontSize: 11 }}>必填</Tag>}
                  </Space>
                  {f.description && (
                    <Text type="secondary" style={{ fontSize: 11 }}>{f.description}</Text>
                  )}
                  {f.default !== undefined && (
                    <Text type="secondary" style={{ fontSize: 11 }}>
                      默认值: {String(f.default)}
                    </Text>
                  )}
                </div>
              </div>
            </div>
          ))}
        </Space>
      )}
    </div>
  );
}

/** Render the structured source form (used by both function_node and answer_node). */
function renderInputSourceForm(
  draftSource: InputSource,
  setDraftSource: (src: InputSource) => void,
  upstreamOptions: { value: string; label: string }[],
  fieldOptions: { value: string; label: string }[],
) {
  return (
    <Space direction="vertical" style={{ width: '100%' }} size="middle">
      <Space>
        <Text>值来源:</Text>
        <Select
          value={draftSource.kind}
          onChange={(v) => {
            if (v === 'upstream') setDraftSource({ kind: 'upstream', node_key: '', field: '' });
            else if (v === 'custom') setDraftSource({ kind: 'custom', value: '' });
            else if (v === 'agent_context')
              setDraftSource({ kind: 'agent_context', category: 'user_input', key: '' });
          }}
          style={{ width: 160 }}
          options={[
            { label: '上游节点', value: 'upstream' },
            { label: '自定义值', value: 'custom' },
            { label: 'AgentContext', value: 'agent_context' },
          ]}
        />
      </Space>
      {draftSource.kind === 'upstream' && (
        <>
          <Select
            value={draftSource.node_key || undefined}
            onChange={(v) => setDraftSource({ kind: 'upstream', node_key: v ?? '', field: '' })}
            placeholder="选择上游节点"
            allowClear
            style={{ width: '100%' }}
            options={upstreamOptions}
          />
          <Select
            value={draftSource.field || undefined}
            onChange={(v) => setDraftSource({ ...draftSource, field: v ?? '' })}
            placeholder={draftSource.node_key ? '选择字段（留空=整个输出）' : '请先选择上游节点'}
            allowClear
            disabled={!draftSource.node_key}
            style={{ width: '100%' }}
            options={fieldOptions}
          />
        </>
      )}
      {draftSource.kind === 'custom' && (
        <Input
          placeholder="自定义默认值 (JSON 或字符串)"
          value={typeof draftSource.value === 'string' ? draftSource.value : JSON.stringify(draftSource.value)}
          onChange={(e) => {
            const raw = e.target.value;
            try {
              setDraftSource({ kind: 'custom', value: JSON.parse(raw) });
            } catch {
              setDraftSource({ kind: 'custom', value: raw });
            }
          }}
        />
      )}
      {draftSource.kind === 'agent_context' && (
        <>
          <Select
            value={draftSource.category}
            onChange={(v) => setDraftSource({ kind: 'agent_context', category: v as AgentContextCategory, key: '', sub_key: undefined })}
            placeholder="选择分类"
            style={{ width: '100%' }}
            options={AGENT_CONTEXT_CATEGORIES}
          />
          {draftSource.category === 'user_input' ? (
            <Select
              value={draftSource.key || undefined}
              onChange={(v) => setDraftSource({ kind: 'agent_context', category: 'user_input', key: v ?? '', sub_key: undefined })}
              placeholder="选择字段"
              style={{ width: '100%' }}
              options={USER_INPUT_FIELDS.map((f) => ({ value: f, label: f }))}
            />
          ) : draftSource.category === 'extensions' ? (
            <>
              <Input
                placeholder="扩展 id"
                value={draftSource.key}
                onChange={(e) => setDraftSource({ ...draftSource, key: e.target.value })}
              />
              <Input
                placeholder="字段名 (data / content_type / reply / render_hints)"
                value={draftSource.sub_key || ''}
                onChange={(e) => setDraftSource({ ...draftSource, sub_key: e.target.value })}
              />
            </>
          ) : (
            <Input
              placeholder="key"
              value={draftSource.key}
              onChange={(e) => setDraftSource({ ...draftSource, key: e.target.value })}
            />
          )}
          {draftSource.category === 'user_input' && draftSource.key === 'metadata' && (
            <Input
              placeholder="metadata 子 key (如 channel / actor_id)"
              value={draftSource.sub_key || ''}
              onChange={(e) => setDraftSource({ ...draftSource, sub_key: e.target.value })}
            />
          )}
        </>
      )}
    </Space>
  );
}

function FunctionNodePanel(props: {
  functionId?: number | null;
  functions: FunctionItem[];
  nodeKey: string;
  onUpdate?: (nodeKey: string, inputMapping: InputSpec) => void;
  upstreamNodes?: Array<{ node_key: string; node_type: string; function_id?: number | null }>;
  nodeOutputFields?: (node_key: string) => { value: string; label: string }[];
  initialMapping?: InputSpec | null;
}) {
  const fn = props.functions.find((f) => f.id === props.functionId);
  // 结构化 InputSpec：与 generate_answer_node 共享同一字段
  const [inputMapping, setInputMapping] = useState<InputSpec>(props.initialMapping ?? {});
  const [editingField, setEditingField] = useState<string | null>(null);
  const [draftSource, setDraftSource] = useState<InputSource>({ kind: 'upstream', node_key: '', field: '' });

  if (!fn) {
    return <Empty description="未找到关联的函数" />;
  }

  const inputFields = parseSchemaToFieldList(fn.input_schema);
  const outputFields = parseSchemaToFieldList(fn.output_schema);

  const upstreamOptions = (props.upstreamNodes || []).map((n) => ({
    value: n.node_key,
    label: `${n.node_key}${n.node_type === 'start_node' ? ' (开始)' : n.node_type === 'function_node' ? ' (函数)' : n.node_type === 'generate_answer_node' ? ' (回答)' : ''}`,
  }));

  const fieldOptions = (props.nodeOutputFields && draftSource.kind === 'upstream')
    ? props.nodeOutputFields(draftSource.node_key)
    : [];

  const handleStartEdit = (fieldName: string) => {
    setEditingField(fieldName);
    setDraftSource(inputMapping[fieldName] ?? { kind: 'upstream', node_key: '', field: '' });
  };

  const handleSaveEdit = () => {
    if (!editingField) return;
    const updated: InputSpec = { ...inputMapping };
    if (draftSource.kind === 'upstream') {
      if (!draftSource.node_key.trim()) {
        void message.error('请选择上游节点');
        return;
      }
      updated[editingField] = {
        kind: 'upstream',
        node_key: draftSource.node_key,
        field: draftSource.field || undefined,
      };
    } else if (draftSource.kind === 'custom') {
      updated[editingField] = { kind: 'custom', value: draftSource.value };
    } else if (draftSource.kind === 'agent_context') {
      if (!draftSource.key.trim()) {
        void message.error('请输入 AgentContext key');
        return;
      }
      updated[editingField] = {
        kind: 'agent_context',
        category: draftSource.category,
        key: draftSource.key,
        sub_key: draftSource.sub_key || undefined,
      };
    }
    setInputMapping(updated);
    setEditingField(null);
    props.onUpdate?.(props.nodeKey, updated);
  };

  const handleCancelEdit = () => {
    setEditingField(null);
  };

  return (
    <Space direction="vertical" style={{ width: '100%' }} size="middle">
      <div>
        <Title level={5}>{fn.name}</Title>
        <Text type="secondary">{fn.identifier}</Text>
      </div>
      {fn.description && <Text>{fn.description}</Text>}

      {/* 输入变量值来源配置（结构化 InputSpec） */}
      <div>
        <Text strong style={{ marginBottom: 8, display: 'block' }}>输入变量配置 ({inputFields.length})</Text>
        <Text type="secondary" style={{ fontSize: 11, display: 'block', marginBottom: 8 }}>
          为每个输入变量指定值来源，运行时自动从前置节点 / AgentContext / 字面量获取
        </Text>
        {inputFields.length === 0 ? (
          <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="无输入变量" style={{ margin: '12px 0' }} />
        ) : (
          <Space direction="vertical" style={{ width: '100%' }} size="small">
            {inputFields.map((f) => {
              const src = inputMapping[f.name];
              const isEditing = editingField === f.name;
              return (
                <div
                  key={f.name}
                  style={{
                    border: isEditing ? '1px solid #1890ff' : '1px solid #f0f0f0',
                    borderRadius: 8,
                    padding: '8px 12px',
                    background: isEditing ? '#e6f7ff' : '#fff',
                  }}
                >
                  {isEditing ? (
                    <Space direction="vertical" style={{ width: '100%' }} size="middle">
                      <Space>
                        <Text strong>{f.name}</Text>
                        <Tag color="blue">{f.type}</Tag>
                        {f.required && <Tag color="red">必填</Tag>}
                      </Space>
                      {renderInputSourceForm(draftSource, setDraftSource, upstreamOptions, fieldOptions)}
                      <Space>
                        <a onClick={handleSaveEdit}>保存修改</a>
                        <a onClick={handleCancelEdit}>取消</a>
                      </Space>
                    </Space>
                  ) : (
                    <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                      <div style={{ flex: 1 }}>
                        <Space size={4}>
                          <Text strong style={{ fontSize: 13 }}>{f.name}</Text>
                          <Tag color="blue" style={{ fontSize: 11 }}>{f.type}</Tag>
                          {f.required && <Tag color="red" style={{ fontSize: 11 }}>必填</Tag>}
                        </Space>
                        {src ? (
                          <div style={{ marginTop: 2 }}>
                            <Tag color="cyan" style={{ fontSize: 11 }}>
                              {src.kind === 'upstream' ? '上游' : src.kind === 'custom' ? '字面量' : 'AgentContext'}
                            </Tag>
                            {src.kind === 'upstream' && (
                              <Text type="secondary" style={{ fontSize: 11 }}>
                                源: {src.node_key}{src.field ? `.${src.field}` : ''}
                              </Text>
                            )}
                            {src.kind === 'custom' && (
                              <Text type="secondary" style={{ fontSize: 11 }}>
                                值: {typeof src.value === 'string' ? src.value : JSON.stringify(src.value)}
                              </Text>
                            )}
                            {src.kind === 'agent_context' && (
                              <Text type="secondary" style={{ fontSize: 11 }}>
                                源: {src.category}.{src.key}{src.sub_key ? `.${src.sub_key}` : ''}
                              </Text>
                            )}
                          </div>
                        ) : (
                          <Text type="warning" style={{ fontSize: 11, marginTop: 2, display: 'block' }}>
                            未配置来源
                          </Text>
                        )}
                      </div>
                      <a onClick={() => handleStartEdit(f.name)}>配置</a>
                    </div>
                  )}
                </div>
              );
            })}
          </Space>
        )}
      </div>

      <SchemaFieldList title="输出 Schema" fields={outputFields} />
    </Space>
  );
}

export function NodeDetailDrawer(props: NodeDetailDrawerProps) {
  const [functions, setFunctions] = useState<FunctionItem[]>([]);

  useEffect(() => {
    if ((props.nodeType === 'function' || props.nodeType === 'generate_answer') && props.open) {
      void (async () => {
        try {
          const list = await listFunctions({ limit: 100 });
          setFunctions(list.items);
        } finally {
        }
      })();
    }
  }, [props.nodeType, props.open]);

  const mergedFunctions = props.allFunctions && props.allFunctions.length > 0
    ? props.allFunctions
    : functions;

  const upstreamNodes = useMemo(() => {
    if (!props.allNodes || !props.allEdges) return [];
    const nodeMap = new Map(props.allNodes.map((n) => [n.id, n]));
    // 构建 source -> targets 的邻接表，用于追溯上游
    const parentMap = new Map<string, string[]>();
    for (const e of props.allEdges) {
      if (!e.source || !e.target) continue;
      const parents = parentMap.get(e.target) || [];
      parents.push(e.source);
      parentMap.set(e.target, parents);
    }
    // BFS 收集所有祖先节点（不含当前节点自身）
    const visited = new Set<string>();
    const queue: string[] = [...(parentMap.get(props.nodeKey) || [])];
    while (queue.length > 0) {
      const key = queue.shift()!;
      if (visited.has(key)) continue;
      visited.add(key);
      const ancestors = parentMap.get(key) || [];
      for (const a of ancestors) {
        if (!visited.has(a)) queue.push(a);
      }
    }
    return Array.from(visited).map((key) => {
      const node = nodeMap.get(key);
      const data = node?.data as Record<string, unknown> | undefined;
      const fnId = data?.function_id as number | null | undefined;
      const nodeType = data?.node_type as string | undefined;
      return {
        node_key: key,
        node_type: nodeType || 'function_node',
        function_id: fnId,
      };
    });
  }, [props.allNodes, props.allEdges, props.nodeKey, mergedFunctions]);

  const nodeOutputFields = useMemo(() => {
    return (nodeKey: string) => {
      if (!props.allNodes) return [];
      const fnMap = new Map(mergedFunctions.map((f) => [f.id, f]));
      const nodeMap = new Map(props.allNodes.map((n) => [n.id, n]));
      const node = nodeMap.get(nodeKey);
      if (!node) return [];
      const data = node.data as Record<string, unknown> | undefined;
      const nodeType = data?.node_type as string | undefined;
      // 生成回答节点：通过 node_type、node_config 或 id 模式识别
      if (nodeType === 'generate_answer_node' || data?.node_config || nodeKey.startsWith('answer_')) {
        return [
          { value: 'answer', label: 'answer' },
          { value: 'model_preset', label: 'model_preset' },
        ];
      }
      if (nodeType === 'start_node' || nodeKey === 'start') {
        // 开始节点的输出字段来自 input_schema 中定义的变量
        const inputSchema = data?.input_schema as Record<string, unknown> | undefined;
        if (inputSchema?.properties) {
          const props_ = inputSchema.properties as Record<string, unknown>;
          return Object.keys(props_).map((key) => ({
            value: key,
            label: key,
          }));
        }
        return [];
      }
      const fnId = data?.function_id as number | null | undefined;
      if (fnId) {
        const fn = fnMap.get(fnId);
        if (fn?.output_schema && typeof fn.output_schema === 'object' && fn.output_schema !== null) {
          const out = fn.output_schema as Record<string, unknown>;
          const props_ = out.properties as Record<string, unknown> | undefined;
          if (props_) {
            return Object.keys(props_).map((key) => ({
              value: key,
              label: key,
            }));
          }
        }
      }
      return [];
    };
  }, [props.allNodes, mergedFunctions]);

  const isAnswer = props.nodeType === 'generate_answer';

  return (
    <Drawer
      title={`节点详情 - ${props.nodeKey}`}
      placement="right"
      width={420}
      open={props.open}
      onClose={props.onClose}
    >
      {props.nodeType === 'start' ? (
        <StartNodePanel
          inputSchema={props.inputSchema}
          onUpdate={props.onUpdateStartNode}
        />
      ) : props.nodeType === 'end' ? (
        <EndNodePanel
          outputSchema={props.outputSchema}
          onUpdate={props.onUpdateEndNode}
        />
      ) : isAnswer ? (
        <AnswerNodePanel
          config={props.answerConfig}
          nodeKey={props.nodeKey}
          onUpdate={props.onUpdateAnswerNode}
          upstreamNodes={upstreamNodes}
          nodeOutputFields={nodeOutputFields}
        />
      ) : (
        <FunctionNodePanel
          functionId={props.functionId}
          functions={mergedFunctions}
          nodeKey={props.nodeKey}
          onUpdate={props.onUpdateFunctionNode}
          upstreamNodes={upstreamNodes}
          nodeOutputFields={nodeOutputFields}
          initialMapping={props.functionInputMapping ?? null}
        />
      )}
    </Drawer>
  );
}
