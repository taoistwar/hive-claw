import { Table, Button, Popconfirm, Tag, Typography, Form, Input, Select, Space, DatePicker } from 'antd';
import { EditOutlined, DeleteOutlined, StopOutlined, CheckOutlined, SearchOutlined, ReloadOutlined } from '@ant-design/icons';
import type { ColumnsType } from 'antd/es/table';
import { Admin, AdminSearchParams } from '../services/admin';
import { useAuth } from '../hooks/useAuth';
import dayjs from 'dayjs';

const { Text } = Typography;
const { RangePicker } = DatePicker;

const ROLE_MAP: Record<number, { label: string; color: string }> = {
  1: { label: '普通管理员', color: 'blue' },
  2: { label: '系统管理员', color: 'orange' },
  3: { label: '超级管理员', color: 'red' },
};

interface AdminTableProps {
  admins: Admin[];
  loading: boolean;
  pagination: { current: number; pageSize: number; total: number };
  onPaginationChange: (page: number, pageSize: number) => void;
  onEdit: (admin: Admin) => void;
  onDelete: (id: number) => void;
  onToggleStatus: (id: number, status: number) => void;
  searchParams: AdminSearchParams;
  onSearchParamsChange: (params: AdminSearchParams) => void;
  onSearch: () => void;
  onReset: () => void;
}

const AdminTable: React.FC<AdminTableProps> = ({
  admins,
  loading,
  pagination,
  onPaginationChange,
  onEdit,
  onDelete,
  onToggleStatus,
  searchParams,
  onSearchParamsChange,
  onSearch,
  onReset,
}) => {
  const { user } = useAuth();
  const [form] = Form.useForm();

  const handleSearch = () => {
    const values = form.getFieldsValue();
    const params: AdminSearchParams = {
      search: values.search || undefined,
      status: values.status,
      role: values.role,
    };

    if (values.created_at_range && values.created_at_range.length === 2) {
      params.created_at_start = values.created_at_range[0].format('YYYY-MM-DD');
      params.created_at_end = values.created_at_range[1].add(1, 'day').format('YYYY-MM-DD');
    }

    if (values.last_login_range && values.last_login_range.length === 2) {
      params.last_login_start = values.last_login_range[0].format('YYYY-MM-DD');
      params.last_login_end = values.last_login_range[1].add(1, 'day').format('YYYY-MM-DD');
    }

    onSearchParamsChange(params);
    onSearch(params);
  };

  const handleResetSearch = () => {
    form.resetFields();
    onReset();
  };

  const columns: ColumnsType<Admin> = [
    {
      title: 'ID',
      dataIndex: 'id',
      key: 'id',
      width: 80,
    },
    {
      title: '手机号',
      dataIndex: 'phone',
      key: 'phone',
      width: 130,
    },
    {
      title: '昵称',
      dataIndex: 'nickname',
      key: 'nickname',
      width: 120,
    },
    {
      title: '角色',
      dataIndex: 'role',
      key: 'role',
      width: 120,
      render: (role: number) => {
        const roleInfo = ROLE_MAP[role];
        return roleInfo ? (
          <Tag color={roleInfo.color}>{roleInfo.label}</Tag>
        ) : (
          <Text>{role}</Text>
        );
      },
    },
    {
      title: '状态',
      dataIndex: 'status',
      key: 'status',
      width: 100,
      render: (status: number) => (
        <Tag color={status === 1 ? 'green' : 'default'}>
          {status === 1 ? '启用' : '禁用'}
        </Tag>
      ),
    },
    {
      title: '创建时间',
      dataIndex: 'created_at',
      key: 'created_at',
      width: 180,
      render: (value: string) => new Date(value).toLocaleString(),
    },
    {
      title: '最后登录',
      dataIndex: 'last_login_at',
      key: 'last_login_at',
      width: 180,
      render: (value: string | null) =>
        value ? new Date(value).toLocaleString() : '未登录',
    },
    {
      title: '操作',
      key: 'action',
      width: 200,
      render: (_: unknown, record: Admin) => {
        if (user?.role === 1) return null;

        return (
          <span>
            <Button
              type="link"
              size="small"
              icon={<EditOutlined />}
              onClick={() => onEdit(record)}
            >
              编辑
            </Button>
            {user?.role === 3 && record.id !== user?.id && record.role !== 3 && (
              <Popconfirm
                title="确认删除"
                description="确定要删除该管理员吗？"
                onConfirm={() => onDelete(record.id)}
                okText="确定"
                cancelText="取消"
              >
                <Button type="link" size="small" danger icon={<DeleteOutlined />}>
                  删除
                </Button>
              </Popconfirm>
            )}
            {record.role !== 3 && (
              <Popconfirm
                title={record.status === 1 ? '确认禁用' : '确认启用'}
                description={`确定要${record.status === 1 ? '禁用' : '启用'}该管理员吗？`}
                onConfirm={() =>
                  onToggleStatus(record.id, record.status === 1 ? 0 : 1)
                }
                okText="确定"
                cancelText="取消"
              >
                <Button
                  type="link"
                  size="small"
                  icon={record.status === 1 ? <StopOutlined /> : <CheckOutlined />}
                >
                  {record.status === 1 ? '禁用' : '启用'}
                </Button>
              </Popconfirm>
            )}
          </span>
        );
      },
    },
  ];

  return (
    <div>
      <Form
        form={form}
        layout="inline"
        onFinish={handleSearch}
        style={{ marginBottom: 16 }}
        aria-label="管理员筛选"
      >
        <Form.Item name="search" label="搜索">
          <Input
            placeholder="搜索手机号/昵称/ID"
            allowClear
            style={{ width: 200 }}
            aria-label="搜索手机号、昵称或 ID"
          />
        </Form.Item>
        <Form.Item name="status" label="状态">
          <Select
            placeholder="状态"
            allowClear
            style={{ width: 100 }}
            aria-label="按状态筛选"
          >
            <Select.Option value={1}>启用</Select.Option>
            <Select.Option value={0}>禁用</Select.Option>
          </Select>
        </Form.Item>
        <Form.Item name="role" label="角色">
          <Select
            placeholder="角色"
            allowClear
            style={{ width: 130 }}
            aria-label="按角色筛选"
          >
            <Select.Option value={1}>普通管理员</Select.Option>
            <Select.Option value={2}>系统管理员</Select.Option>
            <Select.Option value={3}>超级管理员</Select.Option>
          </Select>
        </Form.Item>
        <Form.Item name="created_at_range" label="创建时间">
          <RangePicker
            placeholder={['创建时间起', '创建时间止']}
            aria-label="按创建时间筛选"
          />
        </Form.Item>
        <Form.Item name="last_login_range" label="最后登录">
          <RangePicker
            placeholder={['登录时间起', '登录时间止']}
            aria-label="按最后登录时间筛选"
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
      <Table<Admin>
        columns={columns}
        dataSource={admins}
        rowKey="id"
        loading={loading}
        pagination={{
          current: pagination.current,
          pageSize: pagination.pageSize,
          total: pagination.total,
          showSizeChanger: false,
          showTotal: (total) => `共 ${total} 条`,
          onChange: onPaginationChange,
        }}
      />
    </div>
  );
};

export default AdminTable;
