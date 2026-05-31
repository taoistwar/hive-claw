import { Table, Button, Tag, Typography, Form, Input, Select, Space, DatePicker, Modal, Descriptions } from 'antd';
import { SearchOutlined, ReloadOutlined, EyeOutlined } from '@ant-design/icons';
import type { ColumnsType } from 'antd/es/table';
import { AdminAuditLog, AdminAuditLogSearchParams } from '../services/adminAuditLog';

const { Text } = Typography;
const { RangePicker } = DatePicker;

const OPERATION_MAP: Record<string, { label: string; color: string }> = {
  create: { label: '创建', color: 'green' },
  update: { label: '更新', color: 'blue' },
  delete: { label: '删除', color: 'red' },
  enable: { label: '启用', color: 'cyan' },
  disable: { label: '禁用', color: 'orange' },
};

interface AdminAuditLogTableProps {
  auditLogs: AdminAuditLog[];
  loading: boolean;
  pagination: { current: number; pageSize: number; total: number };
  onPaginationChange: (page: number, pageSize: number) => void;
  onViewDetail: (id: number) => void;
  searchParams: AdminAuditLogSearchParams;
  onSearchParamsChange: (params: AdminAuditLogSearchParams) => void;
  onSearch: (params: AdminAuditLogSearchParams) => void;
  onReset: () => void;
  selectedLog: AdminAuditLog | null;
  detailModalVisible: boolean;
  onCloseDetailModal: () => void;
}

const AdminAuditLogTable: React.FC<AdminAuditLogTableProps> = ({
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
    const params: AdminAuditLogSearchParams = {
      operation: values.operation || undefined,
      operator_id: values.operator_id || undefined,
      target_admin_id: values.target_admin_id || undefined,
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

  const columns: ColumnsType<AdminAuditLog> = [
    {
      title: 'ID',
      dataIndex: 'id',
      key: 'id',
      width: 80,
    },
    {
      title: '操作人',
      dataIndex: 'operator_phone_snapshot',
      key: 'operator_phone_snapshot',
      width: 120,
    },
    {
      title: '操作类型',
      dataIndex: 'operation',
      key: 'operation',
      width: 100,
      render: (value: string) => {
        const info = OPERATION_MAP[value];
        return info ? <Tag color={info.color}>{info.label}</Tag> : <Text>{value}</Text>;
      },
    },
    {
      title: '目标管理员',
      dataIndex: 'target_phone_snapshot',
      key: 'target_phone_snapshot',
      width: 120,
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
      render: (_: unknown, record: AdminAuditLog) => (
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
        aria-label="管理审计日志筛选"
      >
        <Form.Item name="operation" label="操作类型">
          <Select
            placeholder="操作类型"
            allowClear
            style={{ width: 100 }}
            aria-label="按操作类型筛选"
          >
            {Object.entries(OPERATION_MAP).map(([key, { label }]) => (
              <Select.Option key={key} value={key}>{label}</Select.Option>
            ))}
          </Select>
        </Form.Item>
        <Form.Item name="operator_id" label="操作人ID">
          <Input
            placeholder="操作人ID"
            allowClear
            style={{ width: 120 }}
            aria-label="按操作人ID筛选"
          />
        </Form.Item>
        <Form.Item name="target_admin_id" label="目标管理员ID">
          <Input
            placeholder="目标管理员ID"
            allowClear
            style={{ width: 140 }}
            aria-label="按目标管理员ID筛选"
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
      <Table<AdminAuditLog>
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
        title="管理审计日志详情"
        open={detailModalVisible}
        onCancel={onCloseDetailModal}
        footer={null}
        width={800}
      >
        {selectedLog && (
          <Descriptions column={2} bordered>
            <Descriptions.Item label="ID">{selectedLog.id}</Descriptions.Item>
            <Descriptions.Item label="操作类型">
              {OPERATION_MAP[selectedLog.operation]?.label || selectedLog.operation}
            </Descriptions.Item>
            <Descriptions.Item label="操作人ID">
              {selectedLog.operator_id || '-'}
            </Descriptions.Item>
            <Descriptions.Item label="操作人手机号">
              {selectedLog.operator_phone_snapshot || '-'}
            </Descriptions.Item>
            <Descriptions.Item label="目标管理员ID">
              {selectedLog.target_admin_id || '-'}
            </Descriptions.Item>
            <Descriptions.Item label="目标管理员手机号">
              {selectedLog.target_phone_snapshot || '-'}
            </Descriptions.Item>
            <Descriptions.Item label="发生时间" span={2}>
              {new Date(selectedLog.occurred_at).toLocaleString()}
            </Descriptions.Item>
            {selectedLog.detail && (
              <Descriptions.Item label="变更详情" span={2}>
                <pre style={{ whiteSpace: 'pre-wrap', wordBreak: 'break-all', margin: 0 }}>
                  {JSON.stringify(selectedLog.detail, null, 2)}
                </pre>
              </Descriptions.Item>
            )}
          </Descriptions>
        )}
      </Modal>
    </div>
  );
};

export default AdminAuditLogTable;
