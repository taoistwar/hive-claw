import { Table, Button, Tag, Typography, Form, Input, Select, Space, DatePicker, Modal, Descriptions } from 'antd';
import { SearchOutlined, ReloadOutlined, EyeOutlined } from '@ant-design/icons';
import type { ColumnsType } from 'antd/es/table';
import { LoginRecord, LoginRecordSearchParams } from '../services/loginRecord';
import dayjs from 'dayjs';

const { Text } = Typography;
const { RangePicker } = DatePicker;

const FAILURE_REASON_MAP: Record<string, { label: string; color: string }> = {
  WRONG_PASSWORD: { label: '密码错误', color: 'orange' },
  ACCOUNT_DISABLED: { label: '账户已禁用', color: 'red' },
  ACCOUNT_LOCKED: { label: '账户已锁定', color: 'volcano' },
  OTHER: { label: '其他', color: 'default' },
};

interface LoginRecordTableProps {
  loginRecords: LoginRecord[];
  loading: boolean;
  pagination: { current: number; pageSize: number; total: number };
  onPaginationChange: (page: number, pageSize: number) => void;
  onViewDetail: (id: number) => void;
  searchParams: LoginRecordSearchParams;
  onSearchParamsChange: (params: LoginRecordSearchParams) => void;
  onSearch: (params: LoginRecordSearchParams) => void;
  onReset: () => void;
  selectedRecord: LoginRecord | null;
  detailModalVisible: boolean;
  onCloseDetailModal: () => void;
}

const LoginRecordTable: React.FC<LoginRecordTableProps> = ({
  loginRecords,
  loading,
  pagination,
  onPaginationChange,
  onViewDetail,
  searchParams,
  onSearchParamsChange,
  onSearch,
  onReset,
  selectedRecord,
  detailModalVisible,
  onCloseDetailModal,
}) => {
  const [form] = Form.useForm();

  const handleSearch = () => {
    const values = form.getFieldsValue();
    const params: LoginRecordSearchParams = {
      search: values.search || undefined,
      success: values.success !== undefined && values.success !== '' ? values.success : undefined,
      failure_reason: values.failure_reason || undefined,
      ip_address: values.ip_address || undefined,
    };

    if (values.login_at_range && values.login_at_range.length === 2) {
      params.login_at_start = values.login_at_range[0].format('YYYY-MM-DD HH:mm:ss');
      params.login_at_end = values.login_at_range[1].format('YYYY-MM-DD HH:mm:ss');
    }

    onSearchParamsChange(params);
    onSearch(params);
  };

  const handleResetSearch = () => {
    form.resetFields();
    onReset();
  };

  const columns: ColumnsType<LoginRecord> = [
    {
      title: 'ID',
      dataIndex: 'id',
      key: 'id',
      width: 80,
    },
    {
      title: '管理员',
      key: 'admin',
      width: 160,
      render: (_: unknown, record: LoginRecord) => (
        <div>
          <div>{record.admin_nickname_snapshot}</div>
          <Text type="secondary" style={{ fontSize: '12px' }}>{record.admin_phone_snapshot}</Text>
        </div>
      ),
    },
    {
      title: '登录结果',
      dataIndex: 'success',
      key: 'success',
      width: 100,
      render: (value: boolean) => (
        <Tag color={value ? 'green' : 'red'}>{value ? '成功' : '失败'}</Tag>
      ),
    },
    {
      title: '失败原因',
      dataIndex: 'failure_reason',
      key: 'failure_reason',
      width: 120,
      render: (value: string | null) => {
        if (!value) return '-';
        const info = FAILURE_REASON_MAP[value];
        return info ? <Tag color={info.color}>{info.label}</Tag> : <Text>{value}</Text>;
      },
    },
    {
      title: 'IP 地址',
      dataIndex: 'ip_address',
      key: 'ip_address',
      width: 150,
    },
    {
      title: '登录时间',
      dataIndex: 'login_at',
      key: 'login_at',
      width: 180,
      render: (value: string) => new Date(value).toLocaleString(),
    },
    {
      title: '操作',
      key: 'action',
      width: 100,
      render: (_: unknown, record: LoginRecord) => (
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
        aria-label="登录日志筛选"
      >
        <Form.Item name="search" label="搜索">
          <Input
            placeholder="手机号/昵称/IP"
            allowClear
            style={{ width: 160 }}
            aria-label="按手机号、昵称或IP搜索"
          />
        </Form.Item>
        <Form.Item name="success" label="登录结果">
          <Select
            placeholder="登录结果"
            allowClear
            style={{ width: 100 }}
            aria-label="按登录结果筛选"
          >
            <Select.Option value={true}>成功</Select.Option>
            <Select.Option value={false}>失败</Select.Option>
          </Select>
        </Form.Item>
        <Form.Item name="failure_reason" label="失败原因">
          <Select
            placeholder="失败原因"
            allowClear
            style={{ width: 120 }}
            aria-label="按失败原因筛选"
          >
            {Object.entries(FAILURE_REASON_MAP).map(([key, { label }]) => (
              <Select.Option key={key} value={key}>{label}</Select.Option>
            ))}
          </Select>
        </Form.Item>
        <Form.Item name="ip_address" label="IP 地址">
          <Input
            placeholder="IP 地址"
            allowClear
            style={{ width: 150 }}
            aria-label="按IP地址筛选"
          />
        </Form.Item>
        <Form.Item name="login_at_range" label="登录时间">
          <RangePicker
            showTime
            placeholder={['开始时间', '结束时间']}
            aria-label="按登录时间筛选"
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
      <Table<LoginRecord>
        columns={columns}
        dataSource={loginRecords}
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
        title="登录日志详情"
        open={detailModalVisible}
        onCancel={onCloseDetailModal}
        footer={null}
        width={700}
      >
        {selectedRecord && (
          <Descriptions column={2} bordered>
            <Descriptions.Item label="ID">{selectedRecord.id}</Descriptions.Item>
            <Descriptions.Item label="管理员ID">
              {selectedRecord.admin_id || '-'}
            </Descriptions.Item>
            <Descriptions.Item label="管理员昵称" span={2}>
              {selectedRecord.admin_nickname_snapshot}
            </Descriptions.Item>
            <Descriptions.Item label="管理员手机号" span={2}>
              {selectedRecord.admin_phone_snapshot}
            </Descriptions.Item>
            <Descriptions.Item label="登录结果" span={2}>
              <Tag color={selectedRecord.success ? 'green' : 'red'}>
                {selectedRecord.success ? '成功' : '失败'}
              </Tag>
            </Descriptions.Item>
            {selectedRecord.failure_reason && (
              <Descriptions.Item label="失败原因" span={2}>
                {FAILURE_REASON_MAP[selectedRecord.failure_reason]?.label || selectedRecord.failure_reason}
              </Descriptions.Item>
            )}
            <Descriptions.Item label="IP 地址" span={2}>
              {selectedRecord.ip_address}
            </Descriptions.Item>
            <Descriptions.Item label="登录时间" span={2}>
              {new Date(selectedRecord.login_at).toLocaleString()}
            </Descriptions.Item>
          </Descriptions>
        )}
      </Modal>
    </div>
  );
};

export default LoginRecordTable;
