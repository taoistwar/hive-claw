// FunctionTester — 动态根据 input_schema 生成表单并测试 Function (US2 扩展)

import { useState } from 'react';
import {
  Button,
  Form,
  Input,
  InputNumber,
  Modal,
  Select,
  Space,
  Spin,
  Switch,
  Typography,
  message,
} from 'antd';
import { invokeFunction, type FunctionItem } from '../services/function';

const { Text, Paragraph } = Typography;

interface SchemaProperty {
  type?: string;
  description?: string;
  enum?: unknown[];
  default?: unknown;
  [key: string]: unknown;
}

interface FunctionTesterProps {
  functionItem: FunctionItem;
  open: boolean;
  onClose: () => void;
}

/**
 * 从 JSON Schema 中解析 properties，生成表单字段配置
 * 支持类型：string, number, integer, boolean, enum
 */
function parseSchemaProperties(schema: unknown): Array<{ key: string; prop: SchemaProperty }> {
  if (!schema || typeof schema !== 'object') return [];
  const schemaObj = schema as Record<string, unknown>;
  const properties = schemaObj.properties as Record<string, SchemaProperty> | undefined;
  if (!properties) return [];
  return Object.entries(properties).map(([key, prop]) => ({ key, prop }));
}

/** 渲染单个表单字段 */
function renderFormField(key: string, prop: SchemaProperty) {
  const type = prop.type ?? 'string';
  const label = prop.description ? (
    <span>
      {key}
      <Text type="secondary" style={{ marginLeft: 8, fontSize: 12 }}>
        {prop.description}
      </Text>
    </span>
  ) : (
    key
  );

  switch (type) {
    case 'string':
      if (prop.enum && Array.isArray(prop.enum)) {
        return (
          <Form.Item key={key} name={key} label={label} initialValue={prop.default}>
            <Select placeholder={`选择 ${key}`}>
              {prop.enum.map((v) => (
                <Select.Option key={String(v)} value={v}>
                  {String(v)}
                </Select.Option>
              ))}
            </Select>
          </Form.Item>
        );
      }
      return (
        <Form.Item key={key} name={key} label={label} initialValue={prop.default}>
          <Input placeholder={`输入 ${key}`} />
        </Form.Item>
      );

    case 'number':
    case 'integer':
      return (
        <Form.Item key={key} name={key} label={label} initialValue={prop.default}>
          <InputNumber
            placeholder={`输入 ${key}`}
            style={{ width: '100%' }}
            step={type === 'integer' ? 1 : 0.1}
          />
        </Form.Item>
      );

    case 'boolean':
      return (
        <Form.Item key={key} name={key} label={label} valuePropName="checked" initialValue={prop.default}>
          <Switch />
        </Form.Item>
      );

    default:
      return (
        <Form.Item key={key} name={key} label={label} initialValue={prop.default}>
          <Input placeholder={`输入 ${key} (${type})`} />
        </Form.Item>
      );
  }
}

export default function FunctionTester({ functionItem, open, onClose }: FunctionTesterProps) {
  const [form] = Form.useForm();
  const [loading, setLoading] = useState(false);
  const [result, setResult] = useState<{ output: unknown; elapsed_ms: number } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const fields = parseSchemaProperties(functionItem.input_schema);

  const handleTest = async () => {
    try {
      const values = await form.validateFields();
      setLoading(true);
      setError(null);
      setResult(null);

      const resp = await invokeFunction(functionItem.id, { input: values });
      setResult({ output: resp.output, elapsed_ms: resp.elapsed_ms });
      void message.success(`调用成功 (${resp.elapsed_ms}ms)`);
    } catch (e: unknown) {
      if (e && typeof e === 'object' && 'errorFields' in e) {
        void message.error('请检查表单输入');
      } else {
        // 提取后端返回的详细错误信息
        let errMsg = '未知错误';
        if (e && typeof e === 'object' && 'response' in e) {
          const resp = (e as { response?: { data?: { code?: number; message?: string } } }).response;
          if (resp?.data?.message) {
            errMsg = resp.data.message;
          }
        } else if (e instanceof Error) {
          errMsg = e.message;
        }
        setError(errMsg);
        void message.error(`调用失败：${errMsg}`);
      }
    } finally {
      setLoading(false);
    }
  };

  const handleReset = () => {
    form.resetFields();
    setResult(null);
    setError(null);
  };

  return (
    <Modal
      title={`测试 Function「${functionItem.identifier}」`}
      open={open}
      onCancel={onClose}
      width={700}
      footer={[
        <Button key="reset" onClick={handleReset} disabled={loading}>
          重置
        </Button>,
        <Button key="close" onClick={onClose} disabled={loading}>
          关闭
        </Button>,
        <Button key="test" type="primary" onClick={handleTest} loading={loading}>
          执行测试
        </Button>,
      ]}
    >
      <Space direction="vertical" style={{ width: '100%' }} size="large">
        {/* 函数信息 */}
        <div>
          <Text strong>函数信息：</Text>
          <Text type="secondary">
            {functionItem.name} | kind: {functionItem.kind === 1 ? 'builtin' : 'custom'}
          </Text>
          {functionItem.description && (
            <Paragraph type="secondary" style={{ margin: '8px 0 0 0' }}>
              {functionItem.description}
            </Paragraph>
          )}
        </div>

        {/* 输入表单 */}
        <div>
          <Text strong style={{ marginBottom: 8, display: 'block' }}>输入参数：</Text>
          {fields.length > 0 ? (
            <Form form={form} layout="vertical" size="small">
              {fields.map(({ key, prop }) => renderFormField(key, prop))}
            </Form>
          ) : (
            <Text type="secondary">该函数无输入参数（空 schema）</Text>
          )}
        </div>

        {/* 测试结果 */}
        {(result || error) && (
          <div>
            <Text strong style={{ marginBottom: 8, display: 'block' }}>测试结果：</Text>
            {loading && <Spin />}
            {error && (
              <Paragraph type="danger" style={{ background: '#fff1f0', padding: 8, borderRadius: 4 }}>
                <Text type="danger" strong>错误：</Text>
                <Text type="danger">{error}</Text>
              </Paragraph>
            )}
            {result && (
              <div>
                <Text type="success" strong>耗时：{result.elapsed_ms}ms</Text>
                <pre
                  style={{
                    background: '#f5f5f5',
                    padding: 12,
                    borderRadius: 4,
                    marginTop: 8,
                    maxHeight: 300,
                    overflow: 'auto',
                    fontSize: 13,
                    whiteSpace: 'pre-wrap',
                    wordBreak: 'break-word',
                  }}
                >
                  {typeof result.output === 'string'
                    ? result.output
                    : JSON.stringify(result.output, null, 2)}
                </pre>
              </div>
            )}
          </div>
        )}
      </Space>
    </Modal>
  );
}
