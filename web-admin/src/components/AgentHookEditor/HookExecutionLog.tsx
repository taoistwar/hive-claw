import React, { useState } from 'react';
import { Table, Tag, Select, DatePicker, Button, Space, message } from 'antd';
import { SearchOutlined, ReloadOutlined } from '@ant-design/icons';
import type { ColumnsType } from 'antd/es/table';
import {
  listExecutions,
  type HookExecution,
  TRIGGER_POINT_LABELS,
  ACTION_TYPE_LABELS,
  type TriggerPoint,
} from '../../services/agentHook';

interface Props {
  agentId: number;
}

const OUTCOME_OPTIONS = [
  { value: '', label: '全部结果' },
  { value: 'success', label: '成功' },
  { value: 'error', label: '失败' },
  { value: 'timeout', label: '超时' },
  { value: 'skipped', label: '跳过' },
];

const TRIGGER_OPTIONS = [
  { value: '', label: '全部触发点' },
  ...Object.entries(TRIGGER_POINT_LABELS).map(([value, label]) => ({
    value,
    label,
  })),
];

const HookExecutionLog: React.FC<Props> = ({ agentId }) => {
  const [data, setData] = useState<HookExecution[]>([]);
  const [loading, setLoading] = useState(false);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(1);
  const [pageSize] = useState(20);
  const [triggerPoint, setTriggerPoint] = useState('');
  const [outcome, setOutcome] = useState('');
  const [timeRange, setTimeRange] = useState<[string, string] | null>(null);

  const load = async (p: number = 1) => {
    setLoading(true);
    try {
      const params: Record<string, unknown> = {
        page: p,
        page_size: pageSize,
      };
      if (triggerPoint) params.trigger_point = triggerPoint;
      if (outcome) params.outcome = outcome;
      if (timeRange) {
        params.from = timeRange[0];
        params.to = timeRange[1];
      }
      const res = await listExecutions(agentId, params);
      setData(res.items);
      setTotal(res.total);
      setPage(p);
    } catch {
      message.error('加载执行历史失败');
    } finally {
      setLoading(false);
    }
  };

  React.useEffect(() => {
    load();
  }, [agentId]);

  const outcomeColor: Record<string, string> = {
    success: 'green',
    error: 'red',
    timeout: 'orange',
    skipped: 'default',
  };

  const columns: ColumnsType<HookExecution> = [
    {
      title: '时间',
      dataIndex: 'created_at',
      key: 'created_at',
      width: 170,
      render: (v: string) => new Date(v).toLocaleString(),
    },
    {
      title: '触发点',
      dataIndex: 'trigger_point',
      key: 'trigger_point',
      width: 130,
      render: (v: TriggerPoint) => TRIGGER_POINT_LABELS[v] ?? v,
    },
    { title: '动作类型', dataIndex: 'action_type', width: 110 },
    {
      title: '结果',
      dataIndex: 'outcome',
      key: 'outcome',
      width: 80,
      render: (v: string) => <Tag color={outcomeColor[v]}>{v}</Tag>,
    },
    {
      title: '耗时(ms)',
      dataIndex: 'elapsed_ms',
      key: 'elapsed_ms',
      width: 90,
    },
    {
      title: '错误摘要',
      dataIndex: 'error_summary',
      key: 'error_summary',
      ellipsis: true,
      render: (v: string | null) => v || '-',
    },
  ];

  return (
    <div>
      <Space style={{ marginBottom: 16 }} wrap>
        <Select
          style={{ width: 150 }}
          value={triggerPoint}
          onChange={(v) => {
            setTriggerPoint(v);
            load(1);
          }}
          options={TRIGGER_OPTIONS}
        />
        <Select
          style={{ width: 120 }}
          value={outcome}
          onChange={(v) => {
            setOutcome(v);
            load(1);
          }}
          options={OUTCOME_OPTIONS}
        />
        <DatePicker.RangePicker
          showTime
          onChange={(_, dateStrings) => {
            if (dateStrings[0] && dateStrings[1]) {
              setTimeRange([dateStrings[0], dateStrings[1]]);
            } else {
              setTimeRange(null);
            }
          }}
        />
        <Button
          type="primary"
          icon={<SearchOutlined />}
          onClick={() => load(1)}
        >
          查询
        </Button>
        <Button icon={<ReloadOutlined />} onClick={() => load(page)}>
          刷新
        </Button>
      </Space>

      <Table
        columns={columns}
        dataSource={data}
        rowKey="id"
        loading={loading}
        size="small"
        pagination={{
          current: page,
          pageSize,
          total,
          showSizeChanger: false,
          onChange: (p) => load(p),
        }}
        expandable={{
          expandedRowRender: (record) => (
            <pre
              style={{
                maxWidth: 600,
                overflow: 'auto',
                fontSize: 12,
                background: '#f5f5f5',
                padding: 8,
                borderRadius: 4,
              }}
            >
              {JSON.stringify(
                {
                  agent_identifier: record.agent_identifier,
                  session_id: record.session_id,
                  request_id: record.request_id,
                  context: record.context_snapshot,
                  error: record.error_summary,
                },
                null,
                2,
              )}
            </pre>
          ),
        }}
      />
    </div>
  );
};

export default HookExecutionLog;
