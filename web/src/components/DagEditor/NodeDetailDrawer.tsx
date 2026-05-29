import { Drawer, Form, Input, Select, Space, Tag, Typography, Empty, InputNumber, Divider, message } from 'antd';
import { useState, useEffect, useMemo } from 'react';
import { listFunctions, type FunctionItem } from '../../services/function';
import { listModelPresets, type ModelPreset } from '../../services/agent';
import type { AnswerNodeConfig, AnswerNodeVariable } from '../../services/workflow';
import type { Node, Edge } from 'reactflow';

const { Text, Title } = Typography;

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
  const [historyWindow, setHistoryWindow] = useState(props.config?.history_window ?? 5);
  const [variables, setVariables] = useState<AnswerNodeVariable[]>(props.config?.variables || []);
  const [presets, setPresets] = useState<ModelPreset[]>([]);
  const [presetsLoaded, setPresetsLoaded] = useState(false);
  const [addVarOpen, setAddVarOpen] = useState(false);
  const [newVarName, setNewVarName] = useState('');
  const [newVarSource, setNewVarSource] = useState<'upstream' | 'custom'>('upstream');
  const [newVarSourceNode, setNewVarSourceNode] = useState('');
  const [newVarSourceField, setNewVarSourceField] = useState('');
  const [newVarCustomValue, setNewVarCustomValue] = useState('');

  const upstreamOptions = (props.upstreamNodes || []).map((n) => ({
    value: n.node_key,
    label: `${n.node_key}${n.node_type === 'start_node' ? ' (开始)' : n.node_type === 'function_node' ? ' (函数)' : n.node_type === 'generate_answer_node' ? ' (回答)' : ''}`,
  }));

  const fieldOptions = props.nodeOutputFields
    ? props.nodeOutputFields(newVarSourceNode)
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

  const handleAddVar = () => {
    if (!newVarName.trim()) {
      void message.error('请输入变量名');
      return;
    }
    const newVar: AnswerNodeVariable = {
      name: newVarName.trim(),
      value_source: newVarSource,
      ...(newVarSource === 'upstream' ? {
        source_node_key: newVarSourceNode || undefined,
        source_field: newVarSourceField || undefined,
      } : {
        custom_value: newVarCustomValue || undefined,
      }),
    };
    setVariables([...variables, newVar]);
    setNewVarName('');
    setNewVarSource('upstream');
    setNewVarSourceNode('');
    setNewVarSourceField('');
    setNewVarCustomValue('');
    setAddVarOpen(false);
  };

  const handleRemoveVar = (index: number) => {
    setVariables(variables.filter((_, i) => i !== index));
  };

  const handleSave = () => {
    const config: AnswerNodeConfig = {
      system_prompt: systemPrompt,
      model_preset: modelPreset || undefined,
      history_window: historyWindow,
      variables,
    };
    props.onUpdate?.(config);
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
          onChange={(v) => setHistoryWindow(v ?? 5)}
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
          placeholder={`你是一个智能助手，请根据以下内容回答用户问题：\n{{query}}\n\n请用简洁的语言回复。`}
          rows={6}
          maxLength={32000}
          showCount
        />
        <Text type="secondary" style={{ fontSize: 11 }}>
          使用 {'{{变量名}}'} 引用上游节点的输出值
        </Text>
      </Form.Item>

      <Divider />

      <div>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 8 }}>
          <Text strong>输入变量配置 ({variables.length})</Text>
          <a onClick={() => setAddVarOpen(true)}>+ 添加变量</a>
        </div>
        <Text type="secondary" style={{ fontSize: 11, display: 'block', marginBottom: 12 }}>
          变量值在运行时会自动从前置节点的输出中获取。变量名与系统提示词中的 {'{{变量名}}'} 对应。
        </Text>

        {addVarOpen && (
          <div style={{ background: '#fafafa', padding: 12, borderRadius: 8, marginBottom: 12 }}>
            <Space direction="vertical" style={{ width: '100%' }} size="middle">
              <Input
                placeholder="变量名 (英文，如 query)"
                value={newVarName}
                onChange={(e) => setNewVarName(e.target.value.replace(/[^a-zA-Z0-9_]/g, ''))}
              />
              <Space>
                <Text>值来源:</Text>
                <Select
                  value={newVarSource}
                  onChange={setNewVarSource}
                  style={{ width: 120 }}
                  options={[
                    { label: '上游节点', value: 'upstream' },
                    { label: '自定义值', value: 'custom' },
                  ]}
                />
              </Space>
              {newVarSource === 'upstream' && (
                <>
                  <div>
                    <Text type="secondary" style={{ fontSize: 11, display: 'block', marginBottom: 4 }}>
                      上游节点
                    </Text>
                    <Select
                      value={newVarSourceNode || undefined}
                      onChange={(val) => {
                        setNewVarSourceNode(val || '');
                        setNewVarSourceField('');
                      }}
                      placeholder="选择上游节点"
                      allowClear
                      style={{ width: '100%' }}
                      options={upstreamOptions}
                    />
                  </div>
                  <div>
                    <Text type="secondary" style={{ fontSize: 11, display: 'block', marginBottom: 4 }}>
                      输出字段
                    </Text>
                    <Select
                      value={newVarSourceField || undefined}
                      onChange={(val) => setNewVarSourceField(val || '')}
                      placeholder={newVarSourceNode ? '选择字段（留空=整个输出）' : '请先选择上游节点'}
                      allowClear
                      disabled={!newVarSourceNode}
                      style={{ width: '100%' }}
                      options={fieldOptions}
                    />
                  </div>
                </>
              )}
              {newVarSource === 'custom' && (
                <Input
                  placeholder="自定义默认值"
                  value={newVarCustomValue}
                  onChange={(e) => setNewVarCustomValue(e.target.value)}
                />
              )}
              <Space>
                <a onClick={handleAddVar}>确认添加</a>
                <a onClick={() => setAddVarOpen(false)}>取消</a>
              </Space>
            </Space>
          </div>
        )}

        {variables.length === 0 ? (
          <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="暂无变量" style={{ margin: '20px 0' }} />
        ) : (
          <Space direction="vertical" style={{ width: '100%' }} size="small">
            {variables.map((v, idx) => (
              <div
                key={idx}
                style={{
                  border: '1px solid #f0f0f0',
                  borderRadius: 8,
                  padding: '8px 12px',
                  background: '#fff',
                }}
              >
                <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                  <Space direction="vertical" size={0}>
                    <Space>
                      <Text strong>{v.name}</Text>
                      <Tag color="cyan">{v.value_source === 'upstream' ? '上游节点' : '自定义'}</Tag>
                    </Space>
                    {v.value_source === 'upstream' && v.source_node_key && (
                      <Text type="secondary" style={{ fontSize: 11 }}>
                        源: {v.source_node_key}{v.source_field ? `.${v.source_field}` : ''}
                      </Text>
                    )}
                    {v.value_source === 'custom' && v.custom_value && (
                      <Text type="secondary" style={{ fontSize: 11 }}>
                        值: {v.custom_value}
                      </Text>
                    )}
                  </Space>
                  <a onClick={() => handleRemoveVar(idx)} style={{ color: '#ff4d4f' }}>
                    删除
                  </a>
                </div>
              </div>
            ))}
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

function FunctionNodePanel(props: {
  functionId?: number | null;
  functions: FunctionItem[];
}) {
  const fn = props.functions.find((f) => f.id === props.functionId);
  if (!fn) {
    return <Empty description="未找到关联的函数" />;
  }

  return (
    <Space direction="vertical" style={{ width: '100%' }} size="middle">
      <div>
        <Title level={5}>{fn.name}</Title>
        <Text type="secondary">{fn.identifier}</Text>
      </div>
      {fn.description && <Text>{fn.description}</Text>}
      <div>
        <Text strong>输入 Schema:</Text>
        <pre style={{ background: '#fafafa', padding: 8, borderRadius: 4, fontSize: 12, maxHeight: 200, overflow: 'auto', marginTop: 4 }}>
          {JSON.stringify(fn.input_schema, null, 2)}
        </pre>
      </div>
      <div>
        <Text strong>输出 Schema:</Text>
        <pre style={{ background: '#fafafa', padding: 8, borderRadius: 4, fontSize: 12, maxHeight: 200, overflow: 'auto', marginTop: 4 }}>
          {JSON.stringify(fn.output_schema, null, 2)}
        </pre>
      </div>
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
    const upstreamKeys = props.allEdges
      .filter((e) => e.target === props.nodeKey)
      .map((e) => e.source!);
    return upstreamKeys.map((key) => {
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
        />
      )}
    </Drawer>
  );
}
