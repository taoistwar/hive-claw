// RuntimePoolCard — Plugin Pool 健康度卡片 (T147 / T162)

import { useEffect, useState } from 'react';
import { Card, Col, Row, Statistic, Table, Tag, Typography, Alert } from 'antd';
import type { ColumnsType } from 'antd/es/table';
import { getPoolStats, type PerPluginMetrics, type PoolStats } from '../services/capability';

const { Title } = Typography;
const REFRESH_INTERVAL = 30_000;

export function RuntimePoolCard() {
  const [stats, setStats] = useState<PoolStats | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    const fetchOnce = async () => {
      try {
        const s = await getPoolStats();
        if (alive) {
          setStats(s);
          setError(null);
        }
      } catch (e) {
        if (alive) setError((e as Error).message);
      }
    };
    void fetchOnce();
    const t = setInterval(fetchOnce, REFRESH_INTERVAL);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, []);

  if (error) {
    return <Alert type="error" showIcon message={`Pool stats 加载失败：${error}`} />;
  }

  const g = stats?.global;
  const warn = (g?.reset_failures ?? 0) > 0;
  const cacheMissAlert = (g?.cache_misses ?? 0) > 100;

  const cols: ColumnsType<PerPluginMetrics> = [
    { title: 'plugin', dataIndex: 'identifier' },
    { title: 'version', dataIndex: 'version', width: 90 },
    { title: 'idle', dataIndex: 'idle', width: 70 },
    { title: 'in_use', dataIndex: 'in_use', width: 70 },
    {
      title: 'cache_misses',
      dataIndex: 'cache_misses',
      width: 120,
      render: (n: number) =>
        n > 50 ? <Tag color="orange">{n}</Tag> : <span>{n}</span>,
    },
  ];

  return (
    <Card style={{ marginTop: 24 }} aria-label="Plugin Pool 健康度卡片">
      <Title level={5} style={{ marginTop: 0 }}>
        Plugin Pool 健康度
      </Title>
      {warn ? (
        <Alert
          type="error"
          showIcon
          message={`reset_failures = ${g?.reset_failures}，已丢弃实例；请检查 Plugin 端 reset 实现`}
          style={{ marginBottom: 12 }}
        />
      ) : null}
      {cacheMissAlert ? (
        <Alert
          type="warning"
          showIcon
          message={`cache_misses = ${g?.cache_misses}，建议提升 PLUGIN_POOL_MAX_PER_PLUGIN`}
          style={{ marginBottom: 12 }}
        />
      ) : null}
      <Row gutter={16}>
        <Col span={6}>
          <Statistic title="in_use" value={g?.in_use ?? 0} />
        </Col>
        <Col span={6}>
          <Statistic title="idle" value={g?.idle ?? 0} />
        </Col>
        <Col span={6}>
          <Statistic title="created_total" value={g?.created_total ?? 0} />
        </Col>
        <Col span={6}>
          <Statistic title="cache_misses" value={g?.cache_misses ?? 0} />
        </Col>
      </Row>
      <Table<PerPluginMetrics>
        size="small"
        style={{ marginTop: 16 }}
        rowKey="plugin_id"
        columns={cols}
        dataSource={stats?.per_plugin ?? []}
        pagination={false}
        locale={{ emptyText: '暂无 Plugin 实例（pool 未初始化）' }}
      />
    </Card>
  );
}
