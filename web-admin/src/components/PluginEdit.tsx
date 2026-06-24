import { useEffect } from 'react';
import { Button, Form, Input, message, Divider } from 'antd';

import { updatePlugin, type Plugin, type UpdateMeta } from '../services/plugin';
import { CategoryTreeSelect } from './CategoryTreeSelect';
import { TagMultiSelect } from './TagMultiSelect';

function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${(bytes / k ** i).toFixed(2)} ${sizes[i]}`;
}

export interface PluginEditProps {
  plugin: Plugin;
  onUpdated?: (plugin: Plugin) => void;
  onCancel?: () => void;
}

export function PluginEdit({ plugin, onUpdated, onCancel }: PluginEditProps) {
  const [form] = Form.useForm<UpdateMeta>();

  useEffect(() => {
    form.setFieldsValue({
      name: plugin.name,
      description: plugin.description ?? '',
      author: plugin.author ?? '',
      repository_url: plugin.repository_url ?? '',
      category_id: plugin.category_id ?? undefined,
      tag_ids: plugin.tags?.map((t) => t.id) ?? [],
    });
  }, [plugin, form]);

  const onSubmit = async (values: UpdateMeta) => {
    const meta: UpdateMeta = {
      updated_at: plugin.updated_at,
      name: values.name,
      description: values.description || undefined,
      author: values.author || undefined,
      repository_url: values.repository_url || undefined,
      category_id: values.category_id ?? undefined,
      tag_ids: values.tag_ids?.length ? values.tag_ids : undefined,
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
      <div style={{ fontSize: 16, fontWeight: 600, marginBottom: 12 }}>基本信息</div>
      <Form.Item label="ID">
        <Input value={plugin.id} disabled />
      </Form.Item>
      <Form.Item label="identifier">
        <Input value={plugin.identifier} disabled />
      </Form.Item>
      <Form.Item label="runtime">
        <Input value={plugin.runtime} disabled />
      </Form.Item>
      <Form.Item label="version">
        <Input value={plugin.version} disabled />
      </Form.Item>
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
      <Form.Item label="tag_ids" name="tag_ids">
        <TagMultiSelect />
      </Form.Item>

      <Divider style={{ margin: '24px 0 16px' }} />
      <div style={{ fontSize: 16, fontWeight: 600, marginBottom: 12 }}>存储信息</div>
      <div style={{ padding: '12px 16px', background: '#fafafa', borderRadius: 4, marginBottom: 16 }}>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
          <div style={{ display: 'flex', alignItems: 'baseline' }}>
            <span style={{ color: '#888', width: 100, flexShrink: 0 }}>sha256</span>
            <span style={{ fontFamily: 'monospace', fontSize: 11, wordBreak: 'break-all' }}>{plugin.sha256}</span>
          </div>
          <div style={{ display: 'flex', alignItems: 'baseline' }}>
            <span style={{ color: '#888', width: 100, flexShrink: 0 }}>size_bytes</span>
            <span>{formatBytes(plugin.size_bytes)}</span>
          </div>
          <div style={{ display: 'flex', alignItems: 'baseline' }}>
            <span style={{ color: '#888', width: 100, flexShrink: 0 }}>s3_key</span>
            <span style={{ fontFamily: 'monospace', fontSize: 11, wordBreak: 'break-all' }}>{plugin.s3_key}</span>
          </div>
        </div>
      </div>

      <Divider style={{ margin: '24px 0 16px' }} />
      <div style={{ fontSize: 16, fontWeight: 600, marginBottom: 12 }}>exports</div>
      <div style={{ padding: '12px 16px', background: '#fafafa', borderRadius: 4, marginBottom: 16 }}>
        <span style={{ color: '#888' }}>不可编辑（通过插件包上传更新）</span>
      </div>

      <Divider style={{ margin: '24px 0 16px' }} />
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
