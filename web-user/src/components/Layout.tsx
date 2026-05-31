import { Outlet, useNavigate, useLocation } from 'react-router-dom';
import { Layout, Menu, Dropdown, Avatar, Typography, Button } from 'antd';
import {
  DashboardOutlined,
  LogoutOutlined,
  UserOutlined,
  MessageOutlined,
  SunOutlined,
  MoonOutlined,
  LockOutlined,
} from '@ant-design/icons';
import { useAuth } from '../hooks/useAuth';
import { useTheme } from '../hooks/useTheme';

const { Header, Content, Sider } = Layout;
const { Text } = Typography;

const AppLayout: React.FC = () => {
  const { user, logout } = useAuth();
  const { theme, toggleTheme } = useTheme();
  const navigate = useNavigate();
  const location = useLocation();

  const menuItems = [
    {
      key: '/',
      icon: <DashboardOutlined />,
      label: 'Dashboard',
    },
    {
      key: '/chat',
      icon: <MessageOutlined />,
      label: '聊天',
    },
  ];

  const userMenuItems = [
    {
      key: 'change-password',
      icon: <LockOutlined />,
      label: '修改密码',
      onClick: () => {
        navigate('/settings/change-password');
      },
    },
    {
      type: 'divider' as const,
    },
    {
      key: 'logout',
      icon: <LogoutOutlined />,
      label: 'Logout',
      onClick: () => {
        logout();
        navigate('/login');
      },
    },
  ];

  return (
    <Layout style={{ minHeight: '100vh', background: 'var(--bg-primary)' }}>
      <Sider
        theme="dark"
        width={220}
        style={{
          background: 'var(--bg-secondary)',
          borderRight: '1px solid var(--border-subtle)',
        }}
      >
        <div
          style={{
            padding: '20px 16px',
            borderBottom: '1px solid var(--border-subtle)',
            marginBottom: '8px',
          }}
        >
          <Text
            style={{
              color: 'var(--text-primary)',
              fontSize: '18px',
              fontWeight: 700,
              fontFamily: "'Space Grotesk', sans-serif",
              letterSpacing: '-0.5px',
            }}
          >
            用户中心
          </Text>
        </div>
        <Menu
          mode="inline"
          selectedKeys={[location.pathname === '/' ? '/' : location.pathname]}
          items={menuItems}
          onClick={({ key }) => navigate(key)}
          style={{
            background: 'transparent',
            borderRight: 'none',
          }}
        />
      </Sider>

      <Layout>
        <Header
          style={{
            background: 'var(--bg-secondary)',
            borderBottom: '1px solid var(--border-subtle)',
            padding: '0 24px',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'flex-end',
            gap: '16px',
          }}
        >
          <Button
            type="text"
            icon={theme === 'dark' ? <SunOutlined /> : <MoonOutlined />}
            onClick={toggleTheme}
            style={{
              background: 'var(--theme-toggle-bg)',
              color: 'var(--text-secondary)',
              borderRadius: 'var(--radius-sm)',
              width: '36px',
              height: '36px',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              transition: 'all var(--transition-fast)',
            }}
            onMouseEnter={(e) => {
              e.currentTarget.style.background = 'var(--theme-toggle-hover)';
            }}
            onMouseLeave={(e) => {
              e.currentTarget.style.background = 'var(--theme-toggle-bg)';
            }}
          />

          <Dropdown menu={{ items: userMenuItems }} placement="bottomRight">
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: '10px',
                cursor: 'pointer',
                padding: '6px 12px',
                borderRadius: 'var(--radius-sm)',
                transition: 'background var(--transition-fast)',
              }}
              onMouseEnter={(e) => {
                e.currentTarget.style.background = 'rgba(255, 255, 255, 0.05)';
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.background = 'transparent';
              }}
            >
              <Avatar
                size="small"
                icon={<UserOutlined />}
                style={{ background: 'var(--gradient-primary)' }}
              />
              <Text style={{ color: 'var(--text-secondary)', fontSize: '13px' }}>
                {user?.phone}
              </Text>
            </div>
          </Dropdown>
        </Header>

        <Content
          style={{
            padding: '24px',
            background: 'var(--bg-primary)',
          }}
        >
          <Outlet />
        </Content>
      </Layout>
    </Layout>
  );
};

export default AppLayout;
