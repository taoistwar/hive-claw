import {
  Button,
  DatePicker,
  Descriptions,
  Form,
  Input,
  InputNumber,
  Modal,
  Select,
  Space,
  Table,
  Tag,
  Typography,
} from 'antd'
import { EyeOutlined, ReloadOutlined, SearchOutlined } from '@ant-design/icons'
import type { ColumnsType } from 'antd/es/table'
import {
  toRuntimeAuditFilterTimestamp,
  type RuntimeAuditLog,
  type RuntimeAuditLogSearchParams,
} from '../services/runtimeAuditLog'

const { Text } = Typography
const { RangePicker } = DatePicker

const EVENT_TYPES: Record<string, { label: string; color: string }> = {
  capability_call: { label: '能力调用', color: 'blue' },
  capability_denied: { label: '能力拒绝', color: 'red' },
  plugin_invoke: { label: '插件调用', color: 'purple' },
  workflow_node: { label: '工作流节点', color: 'orange' },
  agent_route: { label: 'Agent 路由', color: 'cyan' },
  llm_invoke: { label: 'LLM 调用', color: 'green' },
  llm_fallback: { label: 'LLM 降级', color: 'volcano' },
  runtime_operation: { label: '运行时操作', color: 'default' },
}

const OUTCOMES: Record<string, { label: string; color: string }> = {
  success: { label: '成功', color: 'green' },
  error: { label: '错误', color: 'red' },
  denied: { label: '拒绝', color: 'orange' },
  timeout: { label: '超时', color: 'gold' },
  skipped: { label: '跳过', color: 'default' },
}

interface Props {
  auditLogs: RuntimeAuditLog[]
  loading: boolean
  pagination: { current: number; pageSize: number; total: number }
  onPaginationChange: (page: number, pageSize: number) => void
  onViewDetail: (id: number) => void
  onSearch: (params: RuntimeAuditLogSearchParams) => void
  onReset: () => void
  selectedLog: RuntimeAuditLog | null
  detailModalVisible: boolean
  onCloseDetailModal: () => void
}

function mappedTag(value: string, mapping: Record<string, { label: string; color: string }>) {
  const item = mapping[value]
  return item ? <Tag color={item.color}>{item.label}</Tag> : <Text>{value}</Text>
}

const RuntimeAuditLogTable: React.FC<Props> = ({
  auditLogs,
  loading,
  pagination,
  onPaginationChange,
  onViewDetail,
  onSearch,
  onReset,
  selectedLog,
  detailModalVisible,
  onCloseDetailModal,
}) => {
  const [form] = Form.useForm()

  const submitSearch = () => {
    const values = form.getFieldsValue()
    const params: RuntimeAuditLogSearchParams = {
      event_type: values.event_type || undefined,
      outcome: values.outcome || undefined,
      capability: values.capability?.trim() || undefined,
      request_id: values.request_id?.trim() || undefined,
      session_id: values.session_id ?? undefined,
      agent_id: values.agent_id ?? undefined,
    }
    if (values.occurred_at_range?.length === 2) {
      params.occurred_at_start = toRuntimeAuditFilterTimestamp(values.occurred_at_range[0])
      params.occurred_at_end = toRuntimeAuditFilterTimestamp(values.occurred_at_range[1])
    }
    onSearch(params)
  }

  const resetSearch = () => {
    form.resetFields()
    onReset()
  }

  const columns: ColumnsType<RuntimeAuditLog> = [
    { title: 'ID', dataIndex: 'id', width: 80 },
    {
      title: '事件类型',
      dataIndex: 'event_type',
      width: 130,
      render: (value: string) => mappedTag(value, EVENT_TYPES),
    },
    {
      title: '结果',
      dataIndex: 'outcome',
      width: 90,
      render: (value: string) => mappedTag(value, OUTCOMES),
    },
    {
      title: '能力',
      dataIndex: 'capability',
      width: 140,
      render: (value: string | null) => value || '-',
    },
    {
      title: 'Request ID',
      dataIndex: 'request_id',
      width: 180,
      ellipsis: true,
      render: (value: string | null) => value || '-',
    },
    {
      title: '耗时',
      dataIndex: 'elapsed_ms',
      width: 90,
      render: (value: number | null) => (value === null ? '-' : `${value}ms`),
    },
    {
      title: '发生时间',
      dataIndex: 'occurred_at',
      width: 180,
      render: (value: string) => new Date(value).toLocaleString(),
    },
    {
      title: '操作',
      width: 90,
      render: (_value, record) => (
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
  ]

  return (
    <>
      <Form
        form={form}
        layout="inline"
        onFinish={submitSearch}
        style={{ marginBottom: 16, rowGap: 12 }}
        aria-label="运行时审计日志筛选"
      >
        <Form.Item name="event_type" label="事件类型">
          <Select placeholder="全部" allowClear style={{ width: 130 }}>
            {Object.entries(EVENT_TYPES).map(([value, item]) => (
              <Select.Option key={value} value={value}>
                {item.label}
              </Select.Option>
            ))}
          </Select>
        </Form.Item>
        <Form.Item name="outcome" label="结果">
          <Select placeholder="全部" allowClear style={{ width: 100 }}>
            {Object.entries(OUTCOMES).map(([value, item]) => (
              <Select.Option key={value} value={value}>
                {item.label}
              </Select.Option>
            ))}
          </Select>
        </Form.Item>
        <Form.Item name="capability" label="能力">
          <Input allowClear placeholder="network.http" style={{ width: 150 }} />
        </Form.Item>
        <Form.Item name="request_id" label="Request ID">
          <Input allowClear style={{ width: 180 }} />
        </Form.Item>
        <Form.Item name="session_id" label="Session ID">
          <InputNumber min={1} precision={0} />
        </Form.Item>
        <Form.Item name="agent_id" label="Agent ID">
          <InputNumber min={1} precision={0} />
        </Form.Item>
        <Form.Item name="occurred_at_range" label="发生时间">
          <RangePicker showTime />
        </Form.Item>
        <Form.Item>
          <Space>
            <Button type="primary" htmlType="submit" icon={<SearchOutlined />}>
              搜索
            </Button>
            <Button onClick={resetSearch} icon={<ReloadOutlined />}>
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
        scroll={{ x: 1100 }}
        pagination={{
          ...pagination,
          showSizeChanger: true,
          pageSizeOptions: ['10', '20', '50', '100'],
          showTotal: (total) => `共 ${total} 条`,
          onChange: onPaginationChange,
        }}
      />

      <Modal
        title="运行时审计日志详情"
        open={detailModalVisible}
        onCancel={onCloseDetailModal}
        footer={null}
        width={800}
      >
        {selectedLog && (
          <Descriptions column={2} bordered>
            <Descriptions.Item label="ID">{selectedLog.id}</Descriptions.Item>
            <Descriptions.Item label="事件类型">
              {mappedTag(selectedLog.event_type, EVENT_TYPES)}
            </Descriptions.Item>
            <Descriptions.Item label="结果">
              {mappedTag(selectedLog.outcome, OUTCOMES)}
            </Descriptions.Item>
            <Descriptions.Item label="能力">{selectedLog.capability || '-'}</Descriptions.Item>
            <Descriptions.Item label="Request ID" span={2}>
              {selectedLog.request_id || '-'}
            </Descriptions.Item>
            <Descriptions.Item label="Session ID">
              {selectedLog.session_id ?? '-'}
            </Descriptions.Item>
            <Descriptions.Item label="Agent ID">{selectedLog.agent_id ?? '-'}</Descriptions.Item>
            <Descriptions.Item label="Plugin ID">{selectedLog.plugin_id ?? '-'}</Descriptions.Item>
            <Descriptions.Item label="Function ID">
              {selectedLog.function_id ?? '-'}
            </Descriptions.Item>
            <Descriptions.Item label="耗时">
              {selectedLog.elapsed_ms === null ? '-' : `${selectedLog.elapsed_ms}ms`}
            </Descriptions.Item>
            <Descriptions.Item label="发生时间">
              {new Date(selectedLog.occurred_at).toLocaleString()}
            </Descriptions.Item>
            {selectedLog.error_message && (
              <Descriptions.Item label="错误信息" span={2}>
                {selectedLog.error_message}
              </Descriptions.Item>
            )}
            {selectedLog.payload_summary && (
              <Descriptions.Item label="安全摘要" span={2}>
                <pre
                  style={{
                    whiteSpace: 'pre-wrap',
                    overflowWrap: 'anywhere',
                    margin: 0,
                  }}
                >
                  {JSON.stringify(selectedLog.payload_summary, null, 2)}
                </pre>
              </Descriptions.Item>
            )}
          </Descriptions>
        )}
      </Modal>
    </>
  )
}

export default RuntimeAuditLogTable
