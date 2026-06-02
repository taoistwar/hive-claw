import { Form, Input, Button, App } from 'antd';
import { UserOutlined, LockOutlined } from '@ant-design/icons';
import { useNavigate } from 'react-router-dom';
import { useAuth } from '../hooks/useAuth';

interface LoginFormValues {
  phone: string;
  password: string;
}

const LoginForm: React.FC = () => {
  const { loginAdmin } = useAuth();
  const navigate = useNavigate();
  const [form] = Form.useForm<LoginFormValues>();
  const { message } = App.useApp();

  const onFinishAdmin = async (values: LoginFormValues) => {
    try {
      await loginAdmin(values.phone, values.password);
      message.success('管理员登录成功');
      navigate('/');
    } catch (error: any) {
      message.error(error.response?.data?.message || '登录失败，请检查手机号和密码');
    }
  };

  return (
    <Form<LoginFormValues>
      form={form}
      name="adminLoginForm"
      onFinish={onFinishAdmin}
      autoComplete="off"
      size="large"
      layout="vertical"
    >
      <Form.Item<LoginFormValues>
        name="phone"
        rules={[
          { required: true, message: '请输入手机号' },
          {
            pattern: /^1[3-9]\d{9}$/,
            message: '请输入有效的11位手机号',
          },
        ]}
      >
        <Input
          placeholder="管理员手机号"
          prefix={<UserOutlined style={{ color: 'var(--text-muted)' }} />}
          style={{
            background: 'var(--bg-input)',
            border: '1px solid var(--border-subtle)',
            borderRadius: 'var(--radius-sm)',
            color: 'var(--text-primary)',
            height: '48px',
            transition: 'all var(--transition-fast)',
          }}
          className="login-input"
        />
      </Form.Item>

      <Form.Item<LoginFormValues>
        name="password"
        rules={[{ required: true, message: '请输入密码' }]}
      >
        <Input.Password
          placeholder="密码"
          prefix={<LockOutlined style={{ color: 'var(--text-muted)' }} />}
          style={{
            background: 'var(--bg-input)',
            border: '1px solid var(--border-subtle)',
            borderRadius: 'var(--radius-sm)',
            color: 'var(--text-primary)',
            height: '48px',
            transition: 'all var(--transition-fast)',
          }}
          className="login-input"
        />
      </Form.Item>

      <Form.Item style={{ marginBottom: 0, marginTop: '24px' }}>
        <Button
          type="primary"
          htmlType="submit"
          block
          style={{
            height: '48px',
            borderRadius: 'var(--radius-sm)',
            fontSize: '16px',
            fontWeight: 600,
            letterSpacing: '0.5px',
            background: 'var(--gradient-primary)',
            border: 'none',
            boxShadow: '0 4px 16px rgba(102, 126, 234, 0.3)',
            transition: 'all var(--transition-fast)',
          }}
          onMouseEnter={(e) => {
            e.currentTarget.style.transform = 'translateY(-2px)';
            e.currentTarget.style.boxShadow = '0 6px 24px rgba(102, 126, 234, 0.4)';
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.transform = 'translateY(0)';
            e.currentTarget.style.boxShadow = '0 4px 16px rgba(102, 126, 234, 0.3)';
          }}
        >
          管理员登录
        </Button>
      </Form.Item>
    </Form>
  );
};

export default LoginForm;
