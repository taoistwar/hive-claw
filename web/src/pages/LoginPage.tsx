import { Card, Typography } from 'antd';
import LoginForm from '../components/LoginForm';

const { Title } = Typography;

const LoginPage: React.FC = () => {
  return (
    <div
      style={{
        display: 'flex',
        justifyContent: 'center',
        alignItems: 'center',
        minHeight: '100vh',
        background: '#f0f2f5',
      }}
    >
      <Card
        style={{ width: 400, boxShadow: '0 2px 8px rgba(0, 0, 0, 0.1)' }}
      >
        <Title level={3} style={{ textAlign: 'center', marginBottom: 24 }}>
          管理中心
        </Title>
        <LoginForm />
      </Card>
    </div>
  );
};

export default LoginPage;
