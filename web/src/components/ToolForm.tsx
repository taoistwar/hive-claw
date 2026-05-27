import { Modal, Form, Input, Select, Button, message } from 'antd';
import { ToolItem, CreateTool, UpdateTool } from '../services/tool';

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

const ToolForm: React.FC<ToolFormProps> = ({
  visible,
  editingTool,
  onCancel,
  onCreate,
  onUpdate,
}) => {
  const [form] = Form.useForm();
  const isEditing = !!editingTool;

  const handleFinish = async (values: any) => {
    try {
      const inputSchema = typeof values.input_schema === 'string' 
        ? JSON.parse(values.input_schema) 
        : values.input_schema;
      const outputSchema = typeof values.output_schema === 'string' 
        ? JSON.parse(values.output_schema) 
        : values.output_schema;

      if (isEditing) {
        await onUpdate({
          name: values.name,
          description: values.description,
          input_schema: inputSchema,
          output_schema: outputSchema,
          updated_at: editingTool.updated_at,
        });
      } else {
        await onCreate({
          identifier: values.identifier,
          name: values.name,
          description: values.description,
          kind: values.kind,
          function_id: values.function_id,
          workflow_id: values.workflow_id,
          input_schema: inputSchema,
          output_schema: outputSchema,
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
      destroyOnClose
      width={700}
    >
      <Form
        form={form}
        layout="vertical"
        onFinish={handleFinish}
        initialValues={
          isEditing
            ? {
                identifier: editingTool.identifier,
                name: editingTool.name,
                description: editingTool.description,
                kind: editingTool.kind,
                function_id: editingTool.function_id,
                workflow_id: editingTool.workflow_id,
                input_schema: typeof editingTool.input_schema === 'string'
                  ? editingTool.input_schema
                  : JSON.stringify(editingTool.input_schema, null, 2),
                output_schema: typeof editingTool.output_schema === 'string'
                  ? editingTool.output_schema
                  : JSON.stringify(editingTool.output_schema, null, 2),
              }
            : { kind: 1 }
        }
      >
        <Form.Item
          name="identifier"
          label="标识符"
          rules={[{ required: true, message: '请输入标识符' }]}
        >
          <Input disabled={isEditing} />
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

        <Form.Item shouldUpdate={(prev, curr) => prev.kind !== curr.kind} noStyle>
          {({ getFieldValue }) =>
            getFieldValue('kind') === 1 ? (
              <Form.Item
                name="function_id"
                label="Function ID"
                rules={[{ required: true, message: '请输入Function ID' }]}
              >
                <Input type="number" />
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
