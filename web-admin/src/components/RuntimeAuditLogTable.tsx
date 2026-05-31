import { Table, Button, Tag, Typography, Form, Input, Select, Space, DatePicker, Modal, Descriptions } from 'antd';
import { SearchOutlined, ReloadOutlined, EyeOutlined } from '@ant-design/icons';
import type { ColumnsType } from 'antd/es/table';
import { RuntimeAuditLog, RuntimeAuditLogSearchParams } from '../services/runtimeAuditLog';
import dayjs from 'dayjs';

const { Text } = Typography;
const { RangePicker } = DatePicker;

const EVENT_TYPE_MAP: Record<string, { label: string; color: string }> = {
  capability_call: { label: '能力调用', color: 'blue' },
  capability_denied: { label: '能力拒绝', color: 'red' },
  plugin_invoke: { label: '插件调用', color: 'purple' },
  workflow_node: { label: '工作流节点', color: 'orange' },
  agent_route: { label: 'Agent路由', color: 'cyan' },
  llm_invoke: { label: 'LLM调用', color: 'green' },
  llm_fallback: { label: 'LLM降级', color: 'volcano' },
};

const OUTCOME_MAP: Record<string, { label: string; color: string }> = {
  success: { label: '成功', color: 'green' },
  error: { label: '错误', color: 'red' },
  denied: { label: '拒绝', color: 'orange' },
  timeout: { label: '超时', color: 'gold' },
};

interface RuntimeAuditLogTableProps {
  auditLogs: RuntimeAuditLog[];
  loading: boolean;
  pagination: { current: number; pageSize: number; total: number };
  onPaginationChange: (page: number, pageSize: number) => void;
  onViewDetail: (id: number) => void;
  searchParams: RuntimeAuditLogSearchParams;
  onSearchParamsChange: (params: RuntimeAuditLogSearchParams) => void;
  onSearch: (params: RuntimeAuditLogSearchParams) => void;
  onReset: () => void;
  selectedLog: RuntimeAuditLog | null;
  detailModalVisible: boolean;
  onCloseDetailModal: () => void;
}

const RuntimeAuditLogTable: React.FC<RuntimeAuditLogTableProps> = ({
  auditLogs,
  loading,
  pagination,
  onPaginationChange,
  onViewDetail,
  searchParams,
  onSearchParamsChange,
  onSearch,
  onReset,
  selectedLog,
  detailModalVisible,
  onCloseDetailModal,
}) => {
  const [form] = Form.useForm();

  const handleSearch = () => {
    const values = form.getFieldsValue();
    const params: RuntimeAuditLogSearchParams = {
      event_type: values.event_type || undefined,
      outcome: values.outcome || undefined,
      capability: values.capability || undefined,
      request_id: values.request_id || undefined,
      session_id: values.session_id || undefined,
      agent_id: values.agent_id || undefined,
    };

    if (values.occurred_at_range && values.occurred_at_range.length === 2) {
      params.occurred_at_start = values.occurred_at_range[0].format('YYYY-MM-DD HH:mm:ss');
      params.occurred_at_end = values.occurred_at_range[1].format('YYYY-MM-DD HH:mm:ss');
    }

    onSearchParamsChange(params);
    onSearch(params);
  };

  const handleResetSearch = () => {
    form.resetFields();
    onReset();
  };

  const columns: ColumnsType<RuntimeAuditLog> = [
    {
      title: 'ID',
      dataIndex: 'id',
      key: 'id',
      width: 80,
    },
    {
      title: '事件类型',
      dataIndex: 'event_type',
      key: 'event_type',
      width: 130,
      render: (value: string) => {
        const info = EVENT_TYPE_MAP[value];
        return info ? <Tag color={info.color}>{info.label}</Tag> : <Text>{value}</Text>;
      },
    },
    {
      title: '结果',
      dataIndex: 'outcome',
      key: 'outcome',
      width: 100,
      render: (value: string) => {
        const info = OUTCOME_MAP[value];
        return info ? <Tag color={info.color}>{info.label}</Tag> : <Text>{value}</Text>;
      },
    },
    {
      title: '能力',
      dataIndex: 'capability',
      key: 'capability',
      width: 150,
      render: (value: string | null) => value || '-',
    },
    {
      title: 'Request ID',
      dataIndex: 'request_id',
      key: 'request_id',
      width: 180,
      ellipsis: true,
      render: (value: string | null) => value || '-',
    },
    {
      title: '耗时(ms)',
      dataIndex: 'elapsed_ms',
      key: 'elapsed_ms',
      width: 100,
      render: (value: number | null) => value !== null ? `${value}ms` : '-',
    },
    {
      title: '发生时间',
      dataIndex: 'occurred_at',
      key: 'occurred_at',
      width: 180,
      render: (value: string) => new Date(value).toLocaleString(),
    },
    {
      title: '操作',
      key: 'action',
      width: 100,
      render: (_: unknown, record: RuntimeAuditLog) => (
        <Button
          type="link"
          size="small"
          icon={<EyeOutlined />}
          onClick={() => onViewDetail(record.id)}
        >
          查看
        </Button>
      ),
    },
  ];

  return (
    <div>
      <Form
        form={form}
        layout="inline"
        onFinish={handleSearch}
        style={{ marginBottom: 16 }}
        aria-label="Agent审计日志筛选"
      >
        <Form.Item name="event_type" label="事件类型">
          <Select
            placeholder="事件类型"
            allowClear
            style={{ width: 130 }}
            aria-label="按事件类型筛选"
          >
            {Object.entries(EVENT_TYPE_MAP).map(([key, { label }]) => (
              <Select.Option key={key} value={key}>{label}</Select.Option>
            ))}
          </Select>
        </Form.Item>
        <Form.Item name="outcome" label="结果">
          <Select
            placeholder="结果"
            allowClear
            style={{ width: 100 }}
            aria-label="按结果筛选"
          >
            {Object.entries(OUTCOME_MAP).map(([key, { label }]) => (
              <Select.Option key={key} value={key}>{label}</Select.Option>
            ))}
          </Select>
        </Form.Item>
        <Form.Item name="capability" label="能力">
          <Input
            placeholder="能力名称"
            allowClear
            style={{ width: 150 }}
            aria-label="按能力名称筛选"
          />
        </Form.Item>
        <Form.Item name="request_id" label="Request ID">
          <Input
            placeholder="Request ID"
            allowClear
            style={{ width: 180 }}
            aria-label="按Request ID筛选"
          />
        </Form.Item>
        <Form.Item name="session_id" label="Session ID">
          <Input
            placeholder="Session ID"
            allowClear
            style={{ width: 130 }}
            aria-label="按Session ID筛选"
          />
        </Form.Item>
        <Form.Item name="agent_id" label="Agent ID">
          <Input
            placeholder="Agent ID"
            allowClear
            style={{ width: 130 }}
            aria-label="按Agent ID筛选"
          />
        </Form.Item>
        <Form.Item name="occurred_at_range" label="发生时间">
          <RangePicker
            showTime
            placeholder={['开始时间', '结束时间']}
            aria-label="按发生时间筛选"
          />
        </Form.Item>
        <Form.Item>
          <Space>
            <Button type="primary" htmlType="submit" icon={<SearchOutlined />}>
              搜索
            </Button>
            <Button onClick={handleResetSearch} icon={<ReloadOutlined />}>
              重置
            </Button>
          </Space>
        </Form.Item>
      </Form>
      <Table<RuntimeAuditLog>
        columns={columns}
        dataSource={auditLogs}
        rowKey="id"
        loading={loading}
        pagination={{
          current: pagination.current,
          pageSize: pagination.pageSize,
          total: pagination.total,
          showSizeChanger: true,
          pageSizeOptions: ['10', '20', '50', '100'],
          showTotal: (total) => `共 ${total} 条`,
          onChange: onPaginationChange,
        }}
      />
      <Modal
        title="Agent 审计日志详情"
        open={detailModalVisible}
        onCancel={onCloseDetailModal}
        footer={null}
        width={800}
      >
        {selectedLog && (
          <Descriptions column={2} bordered>
            <Descriptions.Item label="ID">{selectedLog.id}</Descriptions.Item>
            <Descriptions.Item label="事件类型">
              {EVENT_TYPE_MAP[selectedLog.event_type]?.label || selectedLog.event_type}
            </Descriptions.Item>
            <Descriptions.Item label="结果">
              {OUTCOME_MAP[selectedLog.outcome]?.label || selectedLog.outcome}
            </Descriptions.Item>
            <Descriptions.Item label="能力">{selectedLog.capability || '-'}</Descriptions.Item>
            <Descriptions.Item label="Request ID" span={2}>
              {selectedLog.request_id || '-'}
            </Descriptions.Item>
            <Descriptions.Item label="Session ID">
              {selectedLog.session_id || '-'}
            </Descriptions.Item>
            <Descriptions.Item label="Agent ID">
              {selectedLog.agent_id || '-'}
            </Descriptions.Item>
            <Descriptions.Item label="Plugin ID">
              {selectedLog.plugin_id || '-'}
            </Descriptions.Item>
            <Descriptions.Item label="Function ID">
              {selectedLog.function_id || '-'}
            </Descriptions.Item>
            <Descriptions.Item label="耗时(ms)">
              {selectedLog.elapsed_ms !== null ? `${selectedLog.elapsed_ms}ms` : '-'}
            </Descriptions.Item>
            <Descriptions.Item label="发生时间" span={2}>
              {new Date(selectedLog.occurred_at).toLocaleString()}
            </Descriptions.Item>
            {selectedLog.error_message && (
              <Descriptions.Item label="错误信息" span={2}>
                {selectedLog.error_message}
              </Descriptions.Item>
            )}
            {selectedLog.payload_summary && (
              <Descriptions.Item label="Payload摘要" span={2}>
                <pre style={{ whiteSpace: 'pre-wrap', wordBreak: 'break-all', margin: 0 }}>
                  {JSON.stringify(selectedLog.payload_summary, null, 2)}
                </pre>
              </Descriptions.Item>
            )}
          </Descriptions>
        )}
      </Modal>
    </div>
  );
};

export default RuntimeAuditLogTable;
