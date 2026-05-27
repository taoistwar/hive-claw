// FunctionForm — 创建/编辑 Function 的表单组件 (T092 扩展)

import { useState, useEffect } from 'react';
import {
  Form,
  Input,
  Select,
  Button,
  Space,
  Drawer,
  message,
  Spin,
} from 'antd';
import { SchemaEditor } from './SchemaEditor';
import {
  createFunction,
  updateFunction,
  type FunctionItem,
  type CreateFunction,
  type UpdateFunction,
} from '../services/function';
import { listPlugins, type Plugin } from '../services/plugin';
import { listCategoriesFlat, type CategoryItem } from '../services/category';

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

interface CategorySelectOption {
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
  const [categories, setCategories] = useState<CategorySelectOption[]>([]);
  const [loadingOptions, setLoadingOptions] = useState(false);
  const [inputSchema, setInputSchema] = useState<unknown>(defaultInputSchema);
  const [outputSchema, setOutputSchema] = useState<unknown>(defaultOutputSchema);

  useEffect(() => {
    if (!open) return;

    const loadOptions = async () => {
      setLoadingOptions(true);
      try {
        const [pluginRes, categoryRes] = await Promise.all([
          listPlugins({ limit: 1000 }),
          listCategoriesFlat(),
        ]);
        setPlugins(
          pluginRes.items
            .filter((p: Plugin) => !p.deleted_at)
            .map((p: Plugin) => ({
              id: p.id,
              label: `${p.identifier} (${p.version})`,
              value: p.id,
            }))
        );
        setCategories(
          categoryRes.map((c: CategoryItem) => ({
            id: c.id,
            label: c.name,
            value: c.id,
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
      });
      setInputSchema(record.input_schema ?? defaultInputSchema);
      setOutputSchema(record.output_schema ?? defaultOutputSchema);
    } else if (mode === 'create') {
      form.resetFields();
      setInputSchema(defaultInputSchema);
      setOutputSchema(defaultOutputSchema);
    }
  }, [mode, record, form, open]);

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
        };
        await createFunction(payload);
        message.success('函数创建成功');
      } else {
        const payload: UpdateFunction = {
          name: values.name,
          description: values.description,
          category_id: values.category_id,
          input_schema: inputSchema,
          output_schema: outputSchema,
          updated_at: record!.updated_at,
        };
        await updateFunction(record!.id, payload);
        message.success('函数更新成功');
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
      title={mode === 'create' ? '创建函数' : '编辑函数'}
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
            disabled={isBuiltin && mode === 'edit'}
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
        <Form
          form={form}
          layout="vertical"
          disabled={isBuiltin && mode === 'edit'}
        >
          <Form.Item
            name="identifier"
            label="标识符"
            rules={[
              { required: true, message: '请输入函数标识符' },
              {
                pattern: /^[a-zA-Z][a-zA-Z0-9._-]*$/,
                message: '标识符只能包含字母、数字、点、下划线和连字符',
              },
            ]}
          >
            <Input
              placeholder="例如: weather.lookup"
              disabled={mode === 'edit'}
            />
          </Form.Item>

          <Form.Item
            name="name"
            label="名称"
            rules={[{ required: true, message: '请输入函数名称' }]}
          >
            <Input placeholder="例如: 查天气" />
          </Form.Item>

          <Form.Item name="description" label="描述">
            <Input.TextArea rows={2} placeholder="函数描述" />
          </Form.Item>

          {mode === 'create' && (
            <>
              <Form.Item
                name="plugin_id"
                label="关联插件"
                rules={[{ required: true, message: '请选择关联插件' }]}
              >
                <Select
                  placeholder="选择插件"
                  options={plugins}
                  showSearch
                  filterOption={(input, option) =>
                    (option?.label ?? '')
                      .toLowerCase()
                      .includes(input.toLowerCase())
                  }
                />
              </Form.Item>

              <Form.Item
                name="plugin_export"
                label="插件导出函数名"
                rules={[
                  { required: true, message: '请输入插件导出函数名' },
                ]}
              >
                <Input placeholder="例如: lookup" />
              </Form.Item>
            </>
          )}

          <Form.Item name="category_id" label="分类">
            <Select
              placeholder="选择分类"
              allowClear
              options={categories}
              showSearch
              filterOption={(input, option) =>
                (option?.label ?? '')
                  .toLowerCase()
                  .includes(input.toLowerCase())
              }
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
        </Form>
      )}
    </Drawer>
  );
}
