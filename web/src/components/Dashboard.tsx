import { useEffect, useState } from 'react';
import { Row, Col, Card, Statistic, Table, Typography, Spin } from 'antd';
import { TeamOutlined, UserOutlined, LoginOutlined, StopOutlined } from '@ant-design/icons';
import type { ColumnsType } from 'antd/es/table';
import { DashboardStats, LoginRecord, getDashboardStats, getRecentLogins } from '../services/dashboard';

const { Title } = Typography;

const REFRESH_INTERVAL = 30000;

const Dashboard: React.FC = () => {
  const [stats, setStats] = useState<DashboardStats | null>(null);
  const [recentLogins, setRecentLogins] = useState<LoginRecord[]>([]);
  const [loading, setLoading] = useState(true);

  const fetchData = async () => {
    try {
      const [statsData, loginsData] = await Promise.all([
        getDashboardStats(),
        getRecentLogins(10),
      ]);
      setStats(statsData);
      setRecentLogins(loginsData);
    } catch (error) {
      console.error('Failed to fetch dashboard data:', error);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    fetchData();
    const interval = setInterval(fetchData, REFRESH_INTERVAL);
    return () => clearInterval(interval);
  }, []);

  const loginColumns: ColumnsType<LoginRecord> = [
    {
      title: '管理员',
      dataIndex: 'admin_nickname',
      key: 'admin_nickname',
    },
    {
      title: '登录时间',
      dataIndex: 'login_at',
      key: 'login_at',
      render: (value: string) => new Date(value).toLocaleString(),
    },
    {
      title: 'IP地址',
      dataIndex: 'ip_address',
      key: 'ip_address',
    },
    {
      title: '状态',
      dataIndex: 'success',
      key: 'success',
      render: (success: boolean) => (success ? '成功' : '失败'),
    },
  ];

  if (loading) {
    return (
      <div style={{ textAlign: 'center', padding: '48px 0' }}>
        <Spin size="large" />
      </div>
    );
  }

  return (
    <div>
      <Title level={4} style={{ marginBottom: 24 }}>
        仪表盘
      </Title>

      <Row gutter={[16, 16]}>
        <Col xs={24} sm={12} lg={6}>
          <Card>
            <Statistic
              title="管理员总数"
              value={stats?.totalAdmins ?? 0}
              prefix={<TeamOutlined />}
            />
          </Card>
        </Col>
        <Col xs={24} sm={12} lg={6}>
          <Card>
            <Statistic
              title="活跃管理员"
              value={stats?.activeAdmins ?? 0}
              prefix={<UserOutlined />}
              valueStyle={{ color: '#52c41a' }}
            />
          </Card>
        </Col>
        <Col xs={24} sm={12} lg={6}>
          <Card>
            <Statistic
              title="今日登录"
              value={stats?.todayLogins ?? 0}
              prefix={<LoginOutlined />}
              valueStyle={{ color: '#1890ff' }}
            />
          </Card>
        </Col>
        <Col xs={24} sm={12} lg={6}>
          <Card>
            <Statistic
              title="已禁用"
              value={stats?.disabledAdmins ?? 0}
              prefix={<StopOutlined />}
              valueStyle={{ color: '#ff4d4f' }}
            />
          </Card>
        </Col>
      </Row>

      <Card title="最近登录记录" style={{ marginTop: 24 }}>
        <Table<LoginRecord>
          columns={loginColumns}
          dataSource={recentLogins}
          rowKey="id"
          pagination={false}
          size="small"
        />
      </Card>
    </div>
  );
};

export default Dashboard;
