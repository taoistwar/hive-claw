import { Outlet, useNavigate, useLocation } from 'react-router-dom';
import { Layout, Menu, Dropdown, Avatar, Typography } from 'antd';
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
} from '@ant-design/icons';
import { useAuth } from '../hooks/useAuth';

const { Header, Sider, Content } = Layout;
const { Text } = Typography;

const AppLayout: React.FC = () => {
  const { user, logout } = useAuth();
  const navigate = useNavigate();
  const location = useLocation();

  const menuItems = [
    {
      key: '/',
      icon: <DashboardOutlined />,
      label: 'Dashboard',
    },
    ...(user?.role === 1
      ? []
      : user?.role === 2 || user?.role === 3
        ? [
            {
              key: 'business',
              icon: <AppstoreOutlined />,
              label: '业务功能',
              children: [
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
    <Layout style={{ minHeight: '100vh' }}>
      <Sider theme="light" breakpoint="lg" collapsedWidth="80">
        <div style={{ padding: '16px', textAlign: 'center' }}>
          <Text strong style={{ fontSize: '18px' }}>
            Admin Center
          </Text>
        </div>
        <Menu
          mode="inline"
          selectedKeys={[location.pathname]}
          defaultOpenKeys={['business', 'agent-os', 'system']}
          items={menuItems}
          onClick={({ key }) => navigate(key)}
        />
      </Sider>
      <Layout>
        <Header
          style={{
            padding: '0 24px',
            background: '#fff',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'flex-end',
          }}
        >
          <Dropdown menu={{ items: userMenuItems }} placement="bottomRight">
            <div style={{ display: 'flex', alignItems: 'center', cursor: 'pointer' }}>
              <Avatar icon={<UserOutlined />} />
              <Text style={{ marginLeft: '8px' }}>{user?.nickname}</Text>
            </div>
          </Dropdown>
        </Header>
        <Content style={{ margin: '24px 16px', padding: 24, background: '#fff' }}>
          <Outlet />
        </Content>
      </Layout>
    </Layout>
  );
};

export default AppLayout;
