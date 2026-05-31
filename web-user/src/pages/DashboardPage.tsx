import { useEffect, useState } from 'react';
import { Card, Row, Col, Statistic, Spin, Typography } from 'antd';
import {
  MessageOutlined,
  ClockCircleOutlined,
  CalendarOutlined,
  CommentOutlined,
} from '@ant-design/icons';
import { getStats, type DashboardStats } from '../services/dashboard';

const { Title } = Typography;

const DashboardPage: React.FC = () => {
  const [stats, setStats] = useState<DashboardStats | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    getStats()
      .then((data) => setStats(data))
      .catch((err) => {
        console.error('Failed to load dashboard stats:', err);
      })
      .finally(() => setLoading(false));
  }, []);

  if (loading) {
    return (
      <div style={{ display: 'flex', justifyContent: 'center', padding: '80px' }}>
        <Spin size="large" />
      </div>
    );
  }

  return (
    <div>
      <Title
        level={4}
        style={{
          marginBottom: '24px',
          color: 'var(--text-primary)',
          fontFamily: "'Space Grotesk', sans-serif",
        }}
      >
        概览
      </Title>

      <Row gutter={[16, 16]}>
        <Col xs={24} sm={12} lg={6}>
          <Card
            style={{
              background: 'var(--bg-card)',
              border: '1px solid var(--border-subtle)',
              borderRadius: 'var(--radius-md)',
            }}
          >
            <Statistic
              title="总会话数"
              value={stats?.total_sessions ?? 0}
              prefix={<CommentOutlined style={{ color: 'var(--accent-primary)' }} />}
              valueStyle={{ color: 'var(--text-primary)' }}
            />
          </Card>
        </Col>
        <Col xs={24} sm={12} lg={6}>
          <Card
            style={{
              background: 'var(--bg-card)',
              border: '1px solid var(--border-subtle)',
              borderRadius: 'var(--radius-md)',
            }}
          >
            <Statistic
              title="总消息数"
              value={stats?.total_messages ?? 0}
              prefix={<MessageOutlined style={{ color: 'var(--success)' }} />}
              valueStyle={{ color: 'var(--text-primary)' }}
            />
          </Card>
        </Col>
        <Col xs={24} sm={12} lg={6}>
          <Card
            style={{
              background: 'var(--bg-card)',
              border: '1px solid var(--border-subtle)',
              borderRadius: 'var(--radius-md)',
            }}
          >
            <Statistic
              title="活跃天数"
              value={stats?.total_active_days ?? 0}
              prefix={<CalendarOutlined style={{ color: 'var(--warning)' }} />}
              valueStyle={{ color: 'var(--text-primary)' }}
            />
          </Card>
        </Col>
        <Col xs={24} sm={12} lg={6}>
          <Card
            style={{
              background: 'var(--bg-card)',
              border: '1px solid var(--border-subtle)',
              borderRadius: 'var(--radius-md)',
            }}
          >
            <Statistic
              title="最后会话"
              value={stats?.last_session_at ? new Date(stats.last_session_at).toLocaleDateString() : '暂无'}
              prefix={<ClockCircleOutlined style={{ color: 'var(--accent-secondary)' }} />}
              valueStyle={{ color: 'var(--text-primary)', fontSize: '18px' }}
            />
          </Card>
        </Col>
      </Row>
    </div>
  );
};

export default DashboardPage;
