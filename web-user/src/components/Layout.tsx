import { Outlet } from 'react-router-dom';
import { Layout, Button, Typography } from 'antd';
import { MessageOutlined, SunOutlined, MoonOutlined } from '@ant-design/icons';
import { useTheme } from '../hooks/useTheme';

const { Header, Content, Sider } = Layout;
const { Text } = Typography;

const AppLayout: React.FC = () => {
  const { theme, toggleTheme } = useTheme();

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
        <div
          style={{
            padding: '12px 16px',
            display: 'flex',
            alignItems: 'center',
            gap: '10px',
            color: 'var(--accent-primary)',
            background: 'var(--accent-glow)',
            borderRadius: 'var(--radius-sm)',
            margin: '0 8px',
          }}
        >
          <MessageOutlined />
          <Text style={{ color: 'var(--accent-primary)', fontWeight: 500 }}>聊天</Text>
        </div>
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
