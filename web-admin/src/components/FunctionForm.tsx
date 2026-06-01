// FunctionForm — 创建/编辑 Function 的表单组件 (T092 扩展)

import { useState, useEffect } from 'react';
import {
  Form,
  Input,
  Select,
  TreeSelect,
  Button,
  Space,
  Drawer,
  message,
  Spin,
  Tag,
  Card,
} from 'antd';
import type { DataNode } from 'antd/es/tree';
import { SchemaEditor } from './SchemaEditor';
import {
  createFunction,
  updateFunction,
  type FunctionItem,
  type CreateFunction,
  type UpdateFunction,
} from '../services/function';
import { listPlugins, type Plugin, getPluginExports } from '../services/plugin';
import { listCategoriesTree, type CategoryNode } from '../services/category';
import { listCapabilities, type CapabilityItem } from '../services/capability';
import { listTags, type TagItem } from '../services/tag';

interface FunctionFormProps {
  open: boolean;
  mode: 'create' | 'edit';
  record?: FunctionItem | null;
  onClose: () => void;
  onSuccess: () => void;
}

interface PluginSelectOption {
  id: number;
  label: string;
  value: number;
}

const defaultInputSchema = {
  type: 'object',
  properties: {},
  required: [],
};

const defaultOutputSchema = {
  type: 'object',
  properties: {},
  required: [],
};

function buildTreeData(nodes: CategoryNode[]): DataNode[] {
  return nodes.map((n) => ({
    key: n.id,
    value: n.id,
    title: n.name,
    children: n.children?.length ? buildTreeData(n.children) : undefined,
  }));
}

export default function FunctionForm({
  open,
  mode,
  record,
  onClose,
  onSuccess,
}: FunctionFormProps) {
  const [form] = Form.useForm();
  const [submitting, setSubmitting] = useState(false);
  const [plugins, setPlugins] = useState<PluginSelectOption[]>([]);
  const [categoryTreeData, setCategoryTreeData] = useState<DataNode[]>([]);
  const [loadingOptions, setLoadingOptions] = useState(false);
  const [inputSchema, setInputSchema] = useState<unknown>(defaultInputSchema);
  const [outputSchema, setOutputSchema] = useState<unknown>(defaultOutputSchema);
  const [pluginExports, setPluginExports] = useState<string[]>([]);
  const [loadingExports, setLoadingExports] = useState(false);
  const [capabilities, setCapabilities] = useState<CapabilityItem[]>([]);
  const [tags, setTags] = useState<TagItem[]>([]);
  const [loadingTags, setLoadingTags] = useState(false);

  const categorySelector = (
    <TreeSelect
      treeData={categoryTreeData}
      placeholder="选择分类"
      allowClear
      showSearch
      treeNodeFilterProp="title"
      treeDefaultExpandAll
    />
  );

  useEffect(() => {
    if (!open) return;

    const loadOptions = async () => {
      setLoadingOptions(true);
      try {
        const [pluginRes, treeRes, capRes, tagRes] = await Promise.all([
          listPlugins({ limit: 1000 }),
          listCategoriesTree(),
          listCapabilities(),
          listTags(),
        ]);
        setCapabilities(capRes[0]);
        setTags(tagRes.items);
        setCategoryTreeData(buildTreeData(treeRes));
        setPlugins(
          pluginRes.items
            .filter((p: Plugin) => !p.deleted_at)
            .map((p: Plugin) => ({
              id: p.id,
              label: `${p.identifier} (${p.version})`,
              value: p.id,
            }))
        );
      } catch (e) {
        message.error(`加载选项失败：${(e as Error).message}`);
      } finally {
        setLoadingOptions(false);
      }
    };

    loadOptions();
  }, [open]);

  useEffect(() => {
    if (mode === 'edit' && record) {
      form.setFieldsValue({
        identifier: record.identifier,
        name: record.name,
        description: record.description,
        plugin_id: record.plugin_id,
        plugin_export: record.plugin_export,
        category_id: record.category_id,
        required_capabilities: record.required_capabilities || [],
        tag_ids: record.tags?.map((t) => t.id) || [],
      });
      setInputSchema(record.input_schema ?? defaultInputSchema);
      setOutputSchema(record.output_schema ?? defaultOutputSchema);
    } else if (mode === 'create') {
      form.resetFields();
      setInputSchema(defaultInputSchema);
      setOutputSchema(defaultOutputSchema);
      setPluginExports([]);
    }
  }, [mode, record, form, open]);

  /** 选择插件后自动拉取其 WASM 导出函数列表 */
  const handlePluginChange = async (pluginId: number | null) => {
    form.setFieldValue('plugin_export', undefined);
    setPluginExports([]);
    if (!pluginId) return;

    setLoadingExports(true);
    try {
      const exports = await getPluginExports(pluginId);
      setPluginExports(exports);
    } catch (e) {
      message.error(`获取插件导出函数失败：${(e as Error).message}`);
    } finally {
      setLoadingExports(false);
    }
  };

  const handleSubmit = async () => {
    try {
      const values = await form.validateFields();
      setSubmitting(true);

      if (mode === 'create') {
        const payload: CreateFunction = {
          identifier: values.identifier,
          name: values.name,
          description: values.description,
          plugin_id: values.plugin_id,
          plugin_export: values.plugin_export,
          input_schema: inputSchema,
          output_schema: outputSchema,
          category_id: values.category_id,
          required_capabilities: values.required_capabilities,
          tag_ids: values.tag_ids,
        };
        await createFunction(payload);
        message.success('函数创建成功');
      } else {
        const isBuiltin = record?.kind === 1;
        if (isBuiltin) {
          const payload: UpdateFunction = {
            category_id: values.category_id,
            updated_at: record!.updated_at,
            tag_ids: values.tag_ids,
          };
          await updateFunction(record!.id, payload);
          message.success('函数更新成功');
        } else {
          const payload: UpdateFunction = {
            name: values.name,
            description: values.description,
            category_id: values.category_id,
            input_schema: inputSchema,
            output_schema: outputSchema,
            required_capabilities: values.required_capabilities,
            updated_at: record!.updated_at,
            tag_ids: values.tag_ids,
          };
          await updateFunction(record!.id, payload);
          message.success('函数更新成功');
        }
      }

      onSuccess();
      onClose();
    } catch (e) {
      if ((e as Error).name !== 'ValidationError') {
        message.error(`操作失败：${(e as Error).message}`);
      }
    } finally {
      setSubmitting(false);
    }
  };

  const isBuiltin = record?.kind === 1;

  return (
    <Drawer
      title={mode === 'create' ? '创建函数' : isBuiltin ? '编辑分类与标签' : '编辑函数'}
      open={open}
      onClose={onClose}
      width={720}
      extra={
        <Space>
          <Button onClick={onClose}>取消</Button>
          <Button
            type="primary"
            onClick={handleSubmit}
            loading={submitting}
          >
            {mode === 'create' ? '创建' : '保存'}
          </Button>
        </Space>
      }
    >
      {loadingOptions ? (
        <div style={{ textAlign: 'center', padding: '40px 0' }}>
          <Spin size="large" />
        </div>
      ) : (
        <Form form={form} layout="vertical">
          {isBuiltin && mode === 'edit' ? (
            <>
              <Card size="small" style={{ marginBottom: 16 }}>
                <div style={{ color: '#999', fontSize: 12 }}>
                  内置函数仅可编辑分类和标签，其他字段不可修改。
                </div>
              </Card>

              <Form.Item name="identifier" label="标识符">
                <Input disabled />
              </Form.Item>

              <Form.Item name="name" label="名称">
                <Input disabled />
              </Form.Item>

              <Form.Item name="category_id" label="分类">
                {categorySelector}
              </Form.Item>

              <Form.Item name="tag_ids" label="标签" tooltip="为函数添加标签，便于分类和检索">
                <Select
                  mode="multiple"
                  placeholder="选择标签"
                  allowClear
                  options={tags.map((t) => ({
                    label: t.color ? (
                      <Tag color={t.color}>{t.name}</Tag>
                    ) : (
                      t.name
                    ),
                    value: t.id,
                  }))}
                  loading={loadingTags}
                />
              </Form.Item>
            </>
          ) : (
            <>
              {mode === 'edit' && !isBuiltin ? (
                <Card size="small" style={{ marginBottom: 16 }}>
                  <div style={{ color: '#999', fontSize: 12 }}>
                    自定义函数编辑模式下，标识符、关联插件和插件导出函数名不可修改。可修改名称、描述、分类、标签、权限和 Schema。
                  </div>
                </Card>
              ) : null}

              <Form.Item
                name="identifier"
                label="标识符"
                rules={
                  mode === 'create'
                    ? [
                        { required: true, message: '请输入函数标识符' },
                        {
                          pattern: /^[a-zA-Z][a-zA-Z0-9._-]*$/,
                          message: '标识符只能包含字母、数字、点、下划线和连字符',
                        },
                      ]
                    : []
                }
              >
                <Input
                  placeholder="例如: weather.lookup"
                  disabled={mode === 'edit'}
                />
              </Form.Item>

              <Form.Item
                name="name"
                label="名称"
                rules={
                  mode === 'create'
                    ? [{ required: true, message: '请输入函数名称' }]
                    : []
                }
              >
                <Input placeholder="例如: 查天气" />
              </Form.Item>

              <Form.Item name="description" label="描述">
                <Input.TextArea rows={2} placeholder="函数描述" />
              </Form.Item>

              <Form.Item
                name="plugin_id"
                label="关联插件"
                rules={
                  mode === 'create'
                    ? [{ required: true, message: '请选择关联插件' }]
                    : []
                }
              >
                <Select
                  placeholder="选择插件"
                  options={plugins.map((p) => ({
                    ...p,
                    label: p.label,
                    value: p.id,
                  }))}
                  showSearch
                  filterOption={(input, option) =>
                    String(option?.label ?? '')
                      .toLowerCase()
                      .includes(input.toLowerCase())
                  }
                  onChange={handlePluginChange}
                  disabled={mode === 'edit'}
                />
              </Form.Item>

              <Form.Item
                name="plugin_export"
                label="插件导出函数名"
                rules={
                  mode === 'create'
                    ? [{ required: true, message: '请输入插件导出函数名' }]
                    : []
                }
              >
                {mode === 'create' ? (
                  <Select
                    placeholder={
                      loadingExports
                        ? '加载中...'
                        : pluginExports.length > 0
                        ? '选择导出函数'
                        : '请先选择插件'
                    }
                    options={pluginExports.map((exp) => ({ label: exp, value: exp }))}
                    disabled={loadingExports || pluginExports.length === 0}
                    loading={loadingExports}
                    showSearch
                    filterOption={(input, option) =>
                      String(option?.label ?? '')
                        .toLowerCase()
                        .includes(input.toLowerCase())
                    }
                  />
                ) : (
                  <Input disabled />
                )}
              </Form.Item>

              <Form.Item name="category_id" label="分类">
                {categorySelector}
              </Form.Item>

              <Form.Item name="tag_ids" label="标签" tooltip="为函数添加标签，便于分类和检索">
                <Select
                  mode="multiple"
                  placeholder="选择标签"
                  allowClear
                  options={tags.map((t) => ({
                    label: t.color ? (
                      <Tag color={t.color}>{t.name}</Tag>
                    ) : (
                      t.name
                    ),
                    value: t.id,
                  }))}
                  loading={loadingTags}
                />
              </Form.Item>

              <Form.Item name="required_capabilities" label="所需权限" tooltip="该函数执行时需要的能力，执行时会校验 Agent 是否被授权">
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
                  allowClear
                />
              </Form.Item>

              <Form.Item label="输入 Schema" required>
                <SchemaEditor
                  value={inputSchema}
                  onChange={setInputSchema}
                  label="input_schema"
                />
              </Form.Item>

              <Form.Item label="输出 Schema" required>
                <SchemaEditor
                  value={outputSchema}
                  onChange={setOutputSchema}
                  label="output_schema"
                />
              </Form.Item>
            </>
          )}
        </Form>
      )}
    </Drawer>
  );
}
