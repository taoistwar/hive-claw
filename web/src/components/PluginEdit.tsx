import { useEffect } from 'react';
import { Button, Form, Input, message } from 'antd';

import { updatePlugin, type Plugin, type UpdateMeta } from '../services/plugin';
import { CategoryTreeSelect } from './CategoryTreeSelect';

export interface PluginEditProps {
  plugin: Plugin;
  onUpdated?: (plugin: Plugin) => void;
  onCancel?: () => void;
}

export function PluginEdit({ plugin, onUpdated, onCancel }: PluginEditProps) {
  const [form] = Form.useForm<UpdateMeta & { tag_input?: string }>();

  useEffect(() => {
    form.setFieldsValue({
      name: plugin.name,
      description: plugin.description ?? '',
      author: plugin.author ?? '',
      repository_url: plugin.repository_url ?? '',
      category_id: plugin.category_id,
      tag_input: plugin.tags?.map((t) => t.id).join(',') ?? '',
    });
  }, [plugin, form]);

  const onSubmit = async (values: UpdateMeta & { tag_input?: string }) => {
    const tag_ids = (values.tag_input ?? '')
      .split(',')
      .map((s) => Number(s.trim()))
      .filter((n) => !Number.isNaN(n) && n > 0);

    const meta: UpdateMeta = {
      updated_at: plugin.updated_at,
      name: values.name,
      description: values.description || undefined,
      author: values.author || undefined,
      repository_url: values.repository_url || undefined,
      category_id: values.category_id ?? null,
      tag_ids: tag_ids.length ? tag_ids : undefined,
    };

    try {
      const updated = await updatePlugin(plugin.id, meta);
      void message.success('Plugin 更新成功');
      onUpdated?.(updated);
    } catch (e) {
      const err = e as { response?: { data?: { code?: number; message?: string } } };
      const code = err.response?.data?.code;
      if (code === 4094) {
        void message.error('内容已被他人修改，请刷新后重试');
      } else {
        void message.error(`更新失败：${(e as Error).message}`);
      }
    }
  };

  return (
    <Form form={form} layout="vertical" onFinish={onSubmit}>
      <Form.Item label="name" name="name" rules={[{ required: true }]}>
        <Input />
      </Form.Item>
      <Form.Item label="description" name="description">
        <Input.TextArea rows={2} />
      </Form.Item>
      <Form.Item label="author" name="author">
        <Input />
      </Form.Item>
      <Form.Item label="repository_url" name="repository_url">
        <Input />
      </Form.Item>
      <Form.Item label="category_id" name="category_id">
        <CategoryTreeSelect placeholder="选择分类（可选）" />
      </Form.Item>
      <Form.Item label="tag_ids" name="tag_input" tooltip="逗号分隔的标签 ID">
        <Input placeholder="e.g. 1,2,3" />
      </Form.Item>

      <Form.Item>
        <div style={{ display: 'flex', gap: 8 }}>
          <Button type="primary" htmlType="submit">
            保存
          </Button>
          <Button onClick={onCancel}>取消</Button>
        </div>
      </Form.Item>
    </Form>
  );
}
