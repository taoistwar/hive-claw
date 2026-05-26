// PluginUploader — 文件选取 / sha256 预览 / 大小校验 (T075)

import { useState } from 'react';
import { Button, Form, Input, InputNumber, Upload, message } from 'antd';
import { InboxOutlined } from '@ant-design/icons';
import type { UploadFile } from 'antd/es/upload/interface';

import { uploadPlugin, type UploadMeta, type Plugin } from '../services/plugin';

const MAX_BYTES = 16 * 1024 * 1024; // FR-005 v7 + .env PLUGIN_MAX_BYTES default

async function sha256Hex(file: File): Promise<string> {
  const buf = await file.arrayBuffer();
  const digest = await crypto.subtle.digest('SHA-256', buf);
  return Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, '0'))
    .join('');
}

export interface PluginUploaderProps {
  onUploaded?: (plugin: Plugin) => void;
}

export function PluginUploader({ onUploaded }: PluginUploaderProps) {
  const [form] = Form.useForm<UploadMeta>();
  const [fileList, setFileList] = useState<UploadFile[]>([]);
  const [sha256, setSha256] = useState<string>('');
  const [busy, setBusy] = useState(false);

  const beforeUpload = async (file: File) => {
    if (file.size > MAX_BYTES) {
      void message.error(`文件过大：${file.size} > ${MAX_BYTES}`);
      return Upload.LIST_IGNORE;
    }
    setSha256(await sha256Hex(file));
    return false; // 阻止 antd 自动上传；由提交时调 uploadPlugin
  };

  const onSubmit = async (meta: UploadMeta) => {
    const file = fileList[0]?.originFileObj;
    if (!file) {
      void message.error('请先选择 WASM 文件');
      return;
    }
    setBusy(true);
    try {
      const plugin = await uploadPlugin(file, meta);
      void message.success(`Plugin ${plugin.identifier}@${plugin.version} 上传成功`);
      onUploaded?.(plugin);
      form.resetFields();
      setFileList([]);
      setSha256('');
    } catch (e) {
      void message.error(`上传失败：${(e as Error).message}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Form
      form={form}
      layout="vertical"
      onFinish={onSubmit}
      aria-label="上传 Plugin 表单"
    >
      <Form.Item
        label="WASM 文件"
        required
        help={sha256 ? `SHA-256: ${sha256}` : '选择编译产物 *.wasm'}
      >
        <Upload.Dragger
          accept=".wasm,application/wasm"
          beforeUpload={beforeUpload}
          fileList={fileList}
          onChange={({ fileList: fl }) => setFileList(fl)}
          maxCount={1}
        >
          <p className="ant-upload-drag-icon">
            <InboxOutlined />
          </p>
          <p className="ant-upload-text">点击或拖拽 .wasm 文件至此</p>
        </Upload.Dragger>
      </Form.Item>

      <Form.Item label="identifier" name="identifier" rules={[{ required: true }]}>
        <Input placeholder="e.g. weather-tool" />
      </Form.Item>
      <Form.Item label="name" name="name" rules={[{ required: true }]}>
        <Input />
      </Form.Item>
      <Form.Item label="version" name="version" rules={[{ required: true }]}>
        <Input placeholder="e.g. 1.0.0" />
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
        <InputNumber min={1} />
      </Form.Item>

      <Form.Item>
        <Button type="primary" htmlType="submit" loading={busy} disabled={fileList.length === 0}>
          上传
        </Button>
      </Form.Item>
    </Form>
  );
}
