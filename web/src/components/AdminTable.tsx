import { Table, Button, Popconfirm, Tag, Typography } from 'antd';
import { EditOutlined, DeleteOutlined, StopOutlined, CheckOutlined } from '@ant-design/icons';
import type { ColumnsType } from 'antd/es/table';
import { Admin } from '../services/admin';
import { useAuth } from '../hooks/useAuth';

const { Text } = Typography;

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
}

const AdminTable: React.FC<AdminTableProps> = ({
  admins,
  loading,
  pagination,
  onPaginationChange,
  onEdit,
  onDelete,
  onToggleStatus,
}) => {
  const { user } = useAuth();

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
  );
};

export default AdminTable;
