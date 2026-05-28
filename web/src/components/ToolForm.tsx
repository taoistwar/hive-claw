import { useState, useEffect } from 'react';
import { Modal, Form, Input, Select, Button, message, Spin, Switch, Tag } from 'antd';
import { ToolItem, CreateTool, UpdateTool, listTags, TagItem } from '../services/tool';
import { listFunctions, type FunctionItem } from '../services/function';
import { listCategoriesFlat, type CategoryItem } from '../services/category';
import { listCapabilities, type CapabilityItem } from '../services/capability';

interface ToolFormProps {
  visible: boolean;
  editingTool: ToolItem | null;
  onCancel: () => void;
  onCreate: (data: CreateTool) => Promise<void>;
  onUpdate: (data: UpdateTool) => Promise<void>;
}

const KIND_OPTIONS = [
  { value: 1, label: 'Function包装' },
  { value: 2, label: 'Workflow包装' },
];

const SOURCE_OPTIONS = [
  { value: 'workspace', label: 'workspace' },
];

const ToolForm: React.FC<ToolFormProps> = ({
  visible,
  editingTool,
  onCancel,
  onCreate,
  onUpdate,
}) => {
  const [form] = Form.useForm();
  const isEditing = !!editingTool;
  const isBuiltin = editingTool?.source === 'builtin';
  const [functions, setFunctions] = useState<FunctionItem[]>([]);
  const [loadingFunctions, setLoadingFunctions] = useState(false);
  const [tags, setTags] = useState<TagItem[]>([]);
  const [loadingTags, setLoadingTags] = useState(false);
  const [categories, setCategories] = useState<{ id: number; label: string; value: number }[]>([]);
  const [loadingCategories, setLoadingCategories] = useState(false);
  const [capabilities, setCapabilities] = useState<CapabilityItem[]>([]);
  const [loadingCapabilities, setLoadingCapabilities] = useState(false);

  useEffect(() => {
    if (!visible) return;
    setLoadingFunctions(true);
    listFunctions({ limit: 500 })
      .then((res) => setFunctions(res.items))
      .catch(() => {})
      .finally(() => setLoadingFunctions(false));

    setLoadingTags(true);
    listTags()
      .then((res) => setTags(res))
      .catch(() => {})
      .finally(() => setLoadingTags(false));

    setLoadingCategories(true);
    listCategoriesFlat()
      .then((res) => setCategories(res.map((c: CategoryItem) => ({
        id: c.id,
        label: c.name,
        value: c.id,
      }))))
      .catch(() => {})
      .finally(() => setLoadingCategories(false));

    setLoadingCapabilities(true);
    listCapabilities()
      .then((res) => setCapabilities(res))
      .catch(() => {})
      .finally(() => setLoadingCapabilities(false));
  }, [visible]);

  useEffect(() => {
    if (!visible) return;

    if (isEditing && editingTool) {
        form.setFieldsValue({
          identifier: editingTool.identifier,
          name: editingTool.name,
          description: editingTool.description,
          kind: editingTool.kind,
          source: editingTool.source,
          is_always: editingTool.is_always,
          function_id: editingTool.function_id,
          workflow_id: editingTool.workflow_id,
          category_id: editingTool.category_id,
          input_schema: typeof editingTool.input_schema === 'string'
            ? editingTool.input_schema
            : JSON.stringify(editingTool.input_schema, null, 2),
          output_schema: typeof editingTool.output_schema === 'string'
            ? editingTool.output_schema
            : JSON.stringify(editingTool.output_schema, null, 2),
          tags: editingTool.tags.map((t) => t.id),
          required_capabilities: editingTool.required_capabilities || [],
        });
      } else {
        form.resetFields();
        form.setFieldsValue({ kind: 1, is_always: false, tags: [], category_id: undefined, required_capabilities: [] });
      }
  }, [visible, isEditing, editingTool, form]);

  const handleFinish = async (values: any) => {
    try {
      const inputSchema = typeof values.input_schema === 'string'
        ? JSON.parse(values.input_schema)
        : values.input_schema;
      const outputSchema = typeof values.output_schema === 'string'
        ? JSON.parse(values.output_schema)
        : values.output_schema;

      if (isEditing) {
        if (isBuiltin) {
          await onUpdate({
            category_id: values.category_id ?? null,
            tag_ids: values.tags || [],
            is_always: values.is_always || false,
            updated_at: editingTool.updated_at,
          });
        } else {
          await onUpdate({
            name: values.name,
            description: values.description,
            input_schema: inputSchema,
            output_schema: outputSchema,
            category_id: values.category_id ?? null,
            required_capabilities: values.required_capabilities || [],
            tag_ids: values.tags || [],
            is_always: values.is_always || false,
            updated_at: editingTool.updated_at,
          });
        }
      } else {
        await onCreate({
          identifier: values.identifier,
          name: values.name,
          description: values.description,
          kind: values.kind,
          source: values.source || 'workspace',
          is_always: values.is_always || false,
          function_id: values.function_id,
          workflow_id: values.workflow_id,
          input_schema: inputSchema,
          output_schema: outputSchema,
          category_id: values.category_id,
          required_capabilities: values.required_capabilities || [],
          tag_ids: values.tags || [],
        });
      }
      form.resetFields();
    } catch (error: any) {
      message.error(error.message || (isEditing ? '更新工具失败' : '添加工具失败'));
    }
  };

  const validateJson = (_rule: any, value: any) => {
    if (!value) return Promise.resolve();
    try {
      if (typeof value === 'string') {
        JSON.parse(value);
      }
      return Promise.resolve();
    } catch {
      return Promise.reject(new Error('请输入有效的JSON格式'));
    }
  };

  return (
    <Modal
      title={isEditing ? '编辑工具' : '添加工具'}
      open={visible}
      onCancel={onCancel}
      footer={null}
      destroyOnHidden
      width={700}
    >
      <Form
        form={form}
        layout="vertical"
        onFinish={handleFinish}
      >
        {isEditing && isBuiltin && (
          <div style={{ padding: '8px 12px', background: '#fff7e6', border: '1px solid #ffd591', borderRadius: 6, marginBottom: 16, fontSize: 13, color: '#d48806' }}>
            内置工具仅可修改分类、标签和 always 属性
          </div>
        )}

        {!isEditing && (
          <>
            <Form.Item
              name="identifier"
              label="标识符"
              rules={[{ required: true, message: '请输入标识符' }]}
            >
              <Input />
            </Form.Item>

            <Form.Item
              name="name"
              label="名称"
              rules={[{ required: true, message: '请输入名称' }]}
            >
              <Input />
            </Form.Item>

            <Form.Item
              name="description"
              label="描述"
              rules={[{ required: true, message: '请输入描述' }]}
            >
              <Input.TextArea rows={2} />
            </Form.Item>

            <Form.Item
              name="kind"
              label="类型"
              rules={[{ required: true, message: '请选择类型' }]}
            >
              <Select options={KIND_OPTIONS} />
            </Form.Item>

            <Form.Item
              name="source"
              label="来源"
              initialValue="workspace"
              rules={[{ required: true, message: '请选择来源' }]}
            >
              <Select options={SOURCE_OPTIONS} />
            </Form.Item>
          </>
        )}

        {isEditing && !isBuiltin && (
          <>
            <Form.Item
              name="name"
              label="名称"
              rules={[{ required: true, message: '请输入名称' }]}
            >
              <Input />
            </Form.Item>

            <Form.Item
              name="description"
              label="描述"
              rules={[{ required: true, message: '请输入描述' }]}
            >
              <Input.TextArea rows={2} />
            </Form.Item>

            <Form.Item
              name="required_capabilities"
              label="所需权限"
              tooltip="该工具执行时需要的能力，执行时会校验 Agent 是否被授权"
            >
              <Select
                mode="multiple"
                placeholder="选择所需权限"
                options={capabilities.map((c) => ({
                  label: (
                    <span>
                      {c.name}
                      {c.is_dangerous && <Tag color="red" style={{ marginLeft: 4 }}>危险</Tag>}
                    </span>
                  ),
                  value: c.name,
                }))}
                loading={loadingCapabilities}
                allowClear
              />
            </Form.Item>
          </>
        )}

        <Form.Item name="category_id" label="分类">
          <Select
            placeholder="选择分类"
            allowClear
            options={categories}
            loading={loadingCategories}
            showSearch
            filterOption={(input, option) =>
              (option?.label ?? '')
                .toLowerCase()
                .includes(input.toLowerCase())
            }
          />
        </Form.Item>

        <Form.Item
          name="tags"
          label="标签"
        >
          <Select
            mode="multiple"
            placeholder="选择标签"
            options={tags.map((t) => ({
              label: t.name,
              value: t.id,
            }))}
            loading={loadingTags}
            allowClear
          />
        </Form.Item>

        <Form.Item
          name="is_always"
          label="Always"
          valuePropName="checked"
          initialValue={false}
          tooltip="置为 always 时，所有 Agent 都会自动加载该 Tool"
        >
          <Switch />
        </Form.Item>

        {!isBuiltin && (
          <>
            <Form.Item shouldUpdate={(prev, curr) => prev.kind !== curr.kind} noStyle>
              {({ getFieldValue }) =>
                getFieldValue('kind') === 1 ? (
                  <Form.Item
                    name="function_id"
                    label="Function"
                    rules={[{ required: true, message: '请选择Function' }]}
                  >
                    <Select
                      placeholder="选择Function"
                      options={functions.map((f) => ({
                        label: `${f.name}(${f.id})`,
                        value: f.id,
                        description: f.description,
                      }))}
                      showSearch
                      filterOption={(input, option) => {
                        const fn = functions.find((f) => f.id === option?.value);
                        if (!fn) return false;
                        const searchText = `${fn.name} ${fn.description || ''}`.toLowerCase();
                        return searchText.includes(input.toLowerCase());
                      }}
                      disabled={isEditing}
                      loading={loadingFunctions}
                      notFoundContent={loadingFunctions ? <Spin size="small" /> : '无可用Function'}
                      optionRender={(option) => (
                        <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
                          <span>{option.label}</span>
                          <span style={{ fontSize: 12, color: '#999' }}>
                            {option.data?.description || '无描述'}
                          </span>
                        </div>
                      )}
                    />
                  </Form.Item>
                ) : null
              }
            </Form.Item>

            <Form.Item shouldUpdate={(prev, curr) => prev.kind !== curr.kind} noStyle>
              {({ getFieldValue }) =>
                getFieldValue('kind') === 2 ? (
                  <Form.Item
                    name="workflow_id"
                    label="Workflow ID"
                    rules={[{ required: true, message: '请输入Workflow ID' }]}
                  >
                    <Input type="number" />
                  </Form.Item>
                ) : null
              }
            </Form.Item>

            <Form.Item shouldUpdate={(prev, curr) => prev.function_id !== curr.function_id} noStyle>
              {({ getFieldValue }) => {
                const functionId = getFieldValue('function_id');
                const kind = getFieldValue('kind');
                if (kind !== 1 || !functionId) return null;
                const fn = functions.find((f) => f.id === functionId);
                if (!fn) return null;
                return (
                  <div style={{ padding: 12, background: '#f5f5f5', borderRadius: 6, marginBottom: 16 }}>
                    <div style={{ fontSize: 12, color: '#999', marginBottom: 4 }}>Function 描述</div>
                    <div style={{ fontSize: 14, color: '#333' }}>{fn.description || '无描述'}</div>
                  </div>
                );
              }}
            </Form.Item>

            <Form.Item
              name="input_schema"
              label="输入Schema (JSON)"
              rules={[
                { required: true, message: '请输入输入Schema' },
                { validator: validateJson },
              ]}
            >
              <Input.TextArea rows={6} />
            </Form.Item>

            <Form.Item
              name="output_schema"
              label="输出Schema (JSON)"
              rules={[
                { required: true, message: '请输入输出Schema' },
                { validator: validateJson },
              ]}
            >
              <Input.TextArea rows={6} />
            </Form.Item>
          </>
        )}

        {isEditing && isBuiltin && (
          <Form.Item
            name="identifier"
            label="标识符"
          >
            <Input disabled />
          </Form.Item>
        )}

        <Form.Item style={{ marginBottom: 0, textAlign: 'right' }}>
          <Button onClick={onCancel} style={{ marginRight: 8 }}>
            取消
          </Button>
          <Button type="primary" htmlType="submit">
            {isEditing ? '更新' : '添加'}
          </Button>
        </Form.Item>
      </Form>
    </Modal>
  );
};

export default ToolForm;