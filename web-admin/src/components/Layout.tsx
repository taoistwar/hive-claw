import { Outlet, useNavigate, useLocation } from 'react-router-dom';
import { Layout, Menu, Dropdown, Avatar, Typography, Button } from 'antd';
import {
  DashboardOutlined,
  TeamOutlined,
  LogoutOutlined,
  UserOutlined,
  AppstoreOutlined,
  FunctionOutlined,
  ToolOutlined,
  BranchesOutlined,
  BookOutlined,
  FolderOutlined,
  TagsOutlined,
  RobotOutlined,
  StarOutlined,
  ApartmentOutlined,
  FileTextOutlined,
  KeyOutlined,
  MessageOutlined,
  SunOutlined,
  MoonOutlined,
  LockOutlined,
} from '@ant-design/icons';
import { useAuth } from '../hooks/useAuth';
import { useTheme } from '../hooks/useTheme';

const { Header, Content } = Layout;
const { Text } = Typography;

const AppLayout: React.FC = () => {
  const { admin, logout } = useAuth();
  const { theme, toggleTheme } = useTheme();
  const navigate = useNavigate();
  const location = useLocation();

  const menuItems = [
    {
      key: '/',
      icon: <DashboardOutlined />,
      label: 'Dashboard',
    },
    ...(admin?.role === 1
      ? []
      : admin?.role === 2 || admin?.role === 3
        ? [
            {
              key: 'business',
              icon: <AppstoreOutlined />,
              label: '业务功能',
              children: [
                {
                  key: '/users',
                  icon: <TeamOutlined />,
                  label: '用户管理',
                },
                {
                  key: '/recommended-games',
                  icon: <StarOutlined />,
                  label: '推荐游戏',
                },
              ],
            },
            {
              key: 'agent-os',
              icon: <RobotOutlined />,
              label: 'Agent OS',
              children: [
                {
                  key: '/agents',
                  icon: <RobotOutlined />,
                  label: 'Agents',
                },
                {
                  key: '/chat',
                  icon: <MessageOutlined />,
                  label: '聊天',
                },
                {
                  key: '/skills',
                  icon: <BookOutlined />,
                  label: 'Skills',
                },
                {
                  key: '/tools',
                  icon: <ToolOutlined />,
                  label: 'Tools',
                },
                {
                  key: '/workflows',
                  icon: <BranchesOutlined />,
                  label: 'Workflows',
                },
                {
                  key: '/functions',
                  icon: <FunctionOutlined />,
                  label: 'Functions',
                },
                {
                  key: '/plugins',
                  icon: <AppstoreOutlined />,
                  label: 'Plugins',
                },
                {
                  key: '/capabilities',
                  icon: <ApartmentOutlined />,
                  label: 'Capabilities',
                },
                {
                  key: '/categories',
                  icon: <FolderOutlined />,
                  label: 'Categories',
                },
                {
                  key: '/tags',
                  icon: <TagsOutlined />,
                  label: 'Tags',
                },
              ],
            },
            {
              key: 'system',
              icon: <DashboardOutlined />,
              label: '系统管理',
              children: [
                {
                  key: '/admins',
                  icon: <TeamOutlined />,
                  label: '管理员管理',
                },
                {
                  key: '/login-records',
                  icon: <KeyOutlined />,
                  label: '登录日志',
                },
                {
                  key: '/admin-audit-logs',
                  icon: <FileTextOutlined />,
                  label: '管理审计日志',
                },
                {
                  key: '/runtime-audit-logs',
                  icon: <FileTextOutlined />,
                  label: 'Agent 审计日志',
                },
              ],
            },
          ]
        : []),
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
    <Layout style={{ minHeight: '100vh', background: 'var(--bg-primary)', transition: 'background var(--transition-base)' }}>
      <Header
        style={{
          padding: '0 32px',
          background: 'var(--bg-secondary)',
          borderBottom: '1px solid var(--border-subtle)',
          display: 'flex',
          alignItems: 'center',
          gap: '32px',
          position: 'sticky',
          top: 0,
          zIndex: 100,
          width: '100%',
          backdropFilter: 'blur(12px)',
          transition: 'background var(--transition-base), border-color var(--transition-base)',
          overflow: 'visible',
        }}
      >
        {/* Logo */}
        <div style={{ display: 'flex', alignItems: 'center', gap: '12px', whiteSpace: 'nowrap' }}>
          <div
            style={{
              width: '36px',
              height: '36px',
              borderRadius: 'var(--radius-sm)',
              background: 'var(--gradient-primary)',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              boxShadow: '0 2px 8px rgba(102, 126, 234, 0.3)',
            }}
          >
            <svg
              width="20"
              height="20"
              viewBox="0 0 24 24"
              fill="none"
              stroke="white"
              strokeWidth="2.5"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M12 2L2 7l10 5 10-5-10-5z" />
              <path d="M2 17l10 5 10-5" />
              <path d="M2 12l10 5 10-5" />
            </svg>
          </div>
          <Text
            strong
            style={{
              fontSize: '18px',
              fontFamily: "'Space Grotesk', sans-serif",
              color: 'var(--text-primary)',
              letterSpacing: '-0.5px',
              transition: 'color var(--transition-base)',
            }}
          >
            Admin Center
          </Text>
        </div>

        {/* Navigation Menu */}
        <Menu
          mode="horizontal"
          selectedKeys={[location.pathname]}
          items={menuItems}
          onClick={({ key }) => navigate(key)}
          style={{
            border: 'none',
            flex: 1,
            minWidth: 0,
            background: 'transparent',
          }}
        />

        {/* Right side: Theme Toggle + User */}
        <div style={{ display: 'flex', alignItems: 'center', gap: '16px' }}>
          {/* Theme Toggle */}
          <Button
            type="text"
            icon={theme === 'dark' ? <SunOutlined /> : <MoonOutlined />}
            onClick={toggleTheme}
            style={{
              background: 'var(--theme-toggle-bg)',
              border: '1px solid var(--border-subtle)',
              borderRadius: 'var(--radius-sm)',
              color: 'var(--text-secondary)',
              width: '36px',
              height: '36px',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              transition: 'all var(--transition-fast)',
            }}
            onMouseEnter={(e) => {
              e.currentTarget.style.background = 'var(--theme-toggle-hover)';
              e.currentTarget.style.color = 'var(--text-primary)';
            }}
            onMouseLeave={(e) => {
              e.currentTarget.style.background = 'var(--theme-toggle-bg)';
              e.currentTarget.style.color = 'var(--text-secondary)';
            }}
            aria-label={theme === 'dark' ? '切换为亮色主题' : '切换为暗色主题'}
          />

          {/* User Avatar */}
          <Dropdown menu={{ items: userMenuItems }} placement="bottomRight">
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: '12px',
                cursor: 'pointer',
                whiteSpace: 'nowrap',
                padding: '6px 12px',
                borderRadius: 'var(--radius-sm)',
                transition: 'background var(--transition-fast)',
              }}
              onMouseEnter={(e) => {
                e.currentTarget.style.background = 'var(--theme-toggle-hover)';
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.background = 'transparent';
              }}
            >
              <Avatar
                icon={<UserOutlined />}
                style={{
                  background: 'var(--gradient-primary)',
                  border: 'none',
                }}
              />
              <Text style={{ color: 'var(--text-secondary)', fontSize: '14px', transition: 'color var(--transition-base)' }}>
                {admin?.nickname}
              </Text>
            </div>
          </Dropdown>
        </div>
      </Header>
      <Layout style={{ position: 'relative' }}>
        <Content
          style={{
            margin: '24px 32px',
            padding: '32px',
            background: 'var(--bg-secondary)',
            borderRadius: 'var(--radius-lg)',
            border: '1px solid var(--border-subtle)',
            boxShadow: 'var(--shadow-md)',
            minHeight: 'calc(100vh - 120px)',
            animation: 'fadeIn 300ms ease-out',
            transition: 'background var(--transition-base), border-color var(--transition-base)',
            position: 'relative',
            zIndex: 1,
          }}
        >
          <Outlet />
        </Content>
      </Layout>
    </Layout>
  );
};

export default AppLayout;