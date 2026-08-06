import { Modal, Form, Input, Select, Switch, Button, message } from 'antd';
import { Admin, CreateAdminData, UpdateAdminData } from '../services/admin';
import {
  PASSWORD_POLICY_MESSAGE,
  validatePhone,
  validateNickname,
  validatePassword,
} from '../utils/validators';

interface AdminFormProps {
  visible: boolean;
  editingAdmin: Admin | null;
  onCancel: () => void;
  onCreate: (data: CreateAdminData) => Promise<void>;
  onUpdate: (data: UpdateAdminData) => Promise<void>;
}

const ROLE_OPTIONS = [
  { value: 1, label: '普通管理员' },
  { value: 2, label: '系统管理员' },
  { value: 3, label: '超级管理员' },
];

const AdminForm: React.FC<AdminFormProps> = ({
  visible,
  editingAdmin,
  onCancel,
  onCreate,
  onUpdate,
}) => {
  const [form] = Form.useForm();
  const isEditing = !!editingAdmin;

  const handleFinish = async (values: any) => {
    try {
      if (isEditing) {
        await onUpdate({
          nickname: values.nickname,
          role: values.role,
          status: values.status ? 1 : 0,
        });
      } else {
        await onCreate({
          phone: values.phone,
          nickname: values.nickname,
          password: values.password,
          role: values.role,
          status: values.status ? 1 : 0,
        });
      }
      form.resetFields();
    } catch (error: any) {
      if (error.response?.data?.message) {
        message.error(error.response.data.message);
      }
    }
  };

  return (
    <Modal
      title={isEditing ? '编辑管理员' : '添加管理员'}
      open={visible}
      onCancel={onCancel}
      footer={null}
      destroyOnHidden
    >
      <Form
        form={form}
        layout="vertical"
        onFinish={handleFinish}
        initialValues={
          isEditing
            ? {
                phone: editingAdmin.phone,
                nickname: editingAdmin.nickname,
                role: editingAdmin.role,
                status: editingAdmin.status === 1,
              }
            : { status: true, role: 1 }
        }
      >
        <Form.Item
          name="phone"
          label="手机号"
          rules={[
            { required: true, message: '请输入手机号' },
            {
              validator: (_, value) =>
                validatePhone(value)
                  ? Promise.resolve()
                  : Promise.reject(new Error('请输入有效的11位手机号')),
            },
          ]}
        >
          <Input disabled={isEditing} />
        </Form.Item>

        <Form.Item
          name="nickname"
          label="昵称"
          rules={[
            { required: true, message: '请输入昵称' },
            {
              validator: (_, value) =>
                validateNickname(value)
                  ? Promise.resolve()
                  : Promise.reject(new Error('昵称长度必须在1-20个字符之间')),
            },
          ]}
        >
          <Input />
        </Form.Item>

        {!isEditing && (
          <Form.Item
            name="password"
            label="密码"
            rules={[
              { required: true, message: '请输入密码' },
              {
                validator: (_, value) =>
                  typeof value === 'string' && validatePassword(value)
                    ? Promise.resolve()
                    : Promise.reject(new Error(PASSWORD_POLICY_MESSAGE)),
              },
            ]}
          >
            <Input.Password />
          </Form.Item>
        )}

        <Form.Item name="role" label="角色" rules={[{ required: true, message: '请选择角色' }]}>
          <Select options={ROLE_OPTIONS} />
        </Form.Item>

        <Form.Item name="status" label="状态" valuePropName="checked">
          <Switch checkedChildren="启用" unCheckedChildren="禁用" />
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

export default AdminForm;
