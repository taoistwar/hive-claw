import { useEffect, useState } from 'react';
import { Row, Col, Card, Statistic, Table, Typography, Spin } from 'antd';
import { TeamOutlined, UserOutlined, LoginOutlined, StopOutlined } from '@ant-design/icons';
import type { ColumnsType } from 'antd/es/table';
import { DashboardStats, LoginRecord, getDashboardStats, getRecentLogins } from '../services/dashboard';

const { Title, Text } = Typography;

const REFRESH_INTERVAL = 30000;

const CARD_CONFIGS = [
  {
    key: 'totalAdmins',
    title: '管理员总数',
    icon: <TeamOutlined />,
    gradient: 'linear-gradient(135deg, #667eea 0%, #764ba2 100%)',
    shadow: '0 4px 20px rgba(102, 126, 234, 0.2)',
    valueColor: 'var(--accent-primary)',
  },
  {
    key: 'activeAdmins',
    title: '活跃管理员',
    icon: <UserOutlined />,
    gradient: 'linear-gradient(135deg, #48bb78 0%, #38a169 100%)',
    shadow: '0 4px 20px rgba(72, 187, 120, 0.2)',
    valueColor: 'var(--success)',
  },
  {
    key: 'todayLogins',
    title: '今日登录',
    icon: <LoginOutlined />,
    gradient: 'linear-gradient(135deg, #63b3ed 0%, #4299e1 100%)',
    shadow: '0 4px 20px rgba(99, 179, 237, 0.2)',
    valueColor: 'var(--accent-secondary)',
  },
  {
    key: 'disabledAdmins',
    title: '已禁用',
    icon: <StopOutlined />,
    gradient: 'linear-gradient(135deg, #fc8181 0%, #f56565 100%)',
    shadow: '0 4px 20px rgba(252, 129, 129, 0.2)',
    valueColor: 'var(--danger)',
  },
];

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
      render: (success: boolean) => (
        <span
          style={{
            display: 'inline-flex',
            alignItems: 'center',
            gap: '6px',
            padding: '2px 10px',
            borderRadius: '20px',
            fontSize: '12px',
            background: success ? 'var(--success-bg)' : 'var(--danger-bg)',
            color: success ? 'var(--success)' : 'var(--danger)',
          }}
        >
          <span
            style={{
              width: '6px',
              height: '6px',
              borderRadius: '50%',
              background: success ? 'var(--success)' : 'var(--danger)',
            }}
          />
          {success ? '成功' : '失败'}
        </span>
      ),
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
    <div style={{ animation: 'fadeIn 400ms ease-out' }}>
      {/* Page Header */}
      <div style={{ marginBottom: '32px' }}>
        <Title
          level={3}
          style={{
            marginBottom: '8px',
            color: 'var(--text-primary)',
            fontFamily: "'Space Grotesk', sans-serif",
            fontWeight: 600,
            letterSpacing: '-0.5px',
          }}
        >
          仪表盘
        </Title>
        <Text style={{ color: 'var(--text-secondary)', fontSize: '14px' }}>
          实时监控中心，总览系统运行状态
        </Text>
      </div>

      {/* Stats Cards */}
      <Row gutter={[20, 20]}>
        {CARD_CONFIGS.map((config, index) => (
          <Col xs={24} sm={12} lg={6} key={config.key}>
            <Card
              hoverable
              style={{
                background: 'var(--bg-card)',
                border: '1px solid var(--border-subtle)',
                borderRadius: 'var(--radius-md)',
                boxShadow: config.shadow,
                transition: 'all var(--transition-base)',
                overflow: 'hidden',
                position: 'relative',
                animation: 'slideUp 400ms ease-out',
                animationDelay: `${index * 100}ms`,
                animationFillMode: 'both',
              }}
              styles={{ body: { padding: '24px' } }}
              onMouseEnter={(e) => {
                e.currentTarget.style.transform = 'translateY(-4px)';
                e.currentTarget.style.boxShadow = config.shadow.replace('0.2', '0.35');
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.transform = 'translateY(0)';
                e.currentTarget.style.boxShadow = config.shadow;
              }}
            >
              {/* Gradient accent bar */}
              <div
                style={{
                  position: 'absolute',
                  top: 0,
                  left: 0,
                  right: 0,
                  height: '3px',
                  background: config.gradient,
                }}
              />

              <div style={{ display: 'flex', alignItems: 'center', gap: '16px' }}>
                {/* Icon */}
                <div
                  style={{
                    width: '48px',
                    height: '48px',
                    borderRadius: 'var(--radius-sm)',
                    background: config.gradient,
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    fontSize: '20px',
                    color: 'white',
                    boxShadow: config.shadow,
                  }}
                >
                  {config.icon}
                </div>

                {/* Stats */}
                <div>
                  <Statistic
                    title={
                      <span
                        style={{
                          color: 'var(--text-secondary)',
                          fontSize: '13px',
                          fontWeight: 500,
                        }}
                      >
                        {config.title}
                      </span>
                    }
                    value={(stats && stats[config.key as keyof DashboardStats] as number) ?? 0}
                    valueStyle={{
                      color: config.valueColor,
                      fontSize: '28px',
                      fontWeight: 700,
                      fontFamily: "'Space Grotesk', sans-serif",
                    }}
                  />
                </div>
              </div>
            </Card>
          </Col>
        ))}
      </Row>

      {/* Recent Logins Table */}
      <Card
        title={
          <div style={{ display: 'flex', alignItems: 'center', gap: '10px' }}>
            <LoginOutlined style={{ color: 'var(--accent-primary)' }} />
            <span style={{ color: 'var(--text-primary)', fontWeight: 600 }}>最近登录记录</span>
          </div>
        }
        style={{
          marginTop: '24px',
          background: 'var(--bg-card)',
          border: '1px solid var(--border-subtle)',
          borderRadius: 'var(--radius-md)',
          boxShadow: 'var(--shadow-sm)',
          animation: 'slideUp 400ms ease-out 400ms both',
        }}
        styles={{
          header: {
            borderBottom: '1px solid var(--border-subtle)',
            padding: '16px 24px',
          },
          body: { padding: '16px 24px' },
        }}
      >
        <Table<LoginRecord>
          columns={loginColumns}
          dataSource={recentLogins}
          rowKey="id"
          pagination={false}
          size="small"
          style={{ color: 'var(--text-primary)' }}
        />
      </Card>
    </div>
  );
};

export default Dashboard;
