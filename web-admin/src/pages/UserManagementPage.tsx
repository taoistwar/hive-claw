import { useState, useEffect } from 'react';
import { Card, Table, Input, InputNumber, Button, DatePicker, Space, Typography } from 'antd';
import { SearchOutlined } from '@ant-design/icons';
import type { ColumnsType } from 'antd/es/table';
import type { Dayjs } from 'dayjs';
import { getUsers, type UserItem, type ListUsersParams } from '../services/user';

const { Title, Text } = Typography;
const { RangePicker } = DatePicker;

type DateRange = [Dayjs | null, Dayjs | null] | null;

const formatDateTime = (date: string): string => {
  const d = new Date(date);
  return Number.isNaN(d.getTime()) ? '-' : d.toLocaleString('zh-CN');
};

// 选取的 dayjs 范围 -> ISO 字符串；空值返回 undefined，axios 不会拼到 URL 上
const toRangeIso = (range: DateRange): { from?: string; to?: string } => {
  if (!range) return {};
  const [start, end] = range;
  return {
    from: start ? start.toISOString() : undefined,
    to: end ? end.toISOString() : undefined,
  };
};

const UserManagementPage: React.FC = () => {
  const [users, setUsers] = useState<UserItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(10);
  const [idInput, setIdInput] = useState<number | null>(null);
  const [search, setSearch] = useState('');
  const [createdAtRange, setCreatedAtRange] = useState<DateRange>(null);
  const [updatedAtRange, setUpdatedAtRange] = useState<DateRange>(null);

  const fetchUsers = async () => {
    const created = toRangeIso(createdAtRange);
    const updated = toRangeIso(updatedAtRange);
    const params: ListUsersParams = {
      page,
      page_size: pageSize,
      id: idInput ?? undefined,
      search: search.trim() || undefined,
      created_at_from: created.from,
      created_at_to: created.to,
      updated_at_from: updated.from,
      updated_at_to: updated.to,
    };
    setLoading(true);
    try {
      const response = await getUsers(params);
      setUsers(response.users);
      setTotal(response.total);
    } catch (error) {
      console.error('获取用户列表失败', error);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    fetchUsers();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [page, pageSize]);

  const handleSearch = () => {
    setPage(1);
    fetchUsers();
  };

  const columns: ColumnsType<UserItem> = [
    {
      title: 'ID',
      dataIndex: 'id',
      key: 'id',
      width: 80,
    },
    {
      title: 'UID',
      dataIndex: 'uid',
      key: 'uid',
      width: 200,
    },
    {
      title: '昵称',
      dataIndex: 'nickname',
      key: 'nickname',
    },
    {
      title: '首次使用',
      dataIndex: 'created_at',
      key: 'created_at',
      width: 200,
      render: formatDateTime,
    },
    {
      title: '最近使用',
      dataIndex: 'updated_at',
      key: 'updated_at',
      width: 200,
      render: formatDateTime,
    },
  ];

  return (
    <div>
      <div style={{ marginBottom: 16 }}>
        <Title level={4} style={{ margin: 0 }}>用户管理</Title>
        <Text type="secondary" style={{ marginLeft: 12, fontSize: 14 }}>
          浏览使用了助手服务的用户
        </Text>
      </div>

      <Card
        style={{
          background: 'var(--bg-card)',
          border: '1px solid var(--border-subtle)',
          borderRadius: 'var(--radius-lg)',
          boxShadow: 'var(--shadow-md)',
        }}
        styles={{ body: { padding: '16px' } }}
      >
        <Space size={[8, 12]} wrap style={{ marginBottom: 16 }}>
          <InputNumber
            placeholder="ID (精确)"
            value={idInput}
            onChange={(v) => setIdInput(typeof v === 'number' ? v : null)}
            onPressEnter={handleSearch}
            style={{ width: 160 }}
            min={1}
            allowClear
          />
          <Input
            placeholder="搜索 UID / 昵称"
            prefix={<SearchOutlined />}
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            onPressEnter={handleSearch}
            style={{ width: 240 }}
            allowClear
          />
          <RangePicker
            showTime
            placeholder={['首次使用 开始', '首次使用 结束']}
            value={createdAtRange}
            onChange={setCreatedAtRange}
          />
          <RangePicker
            showTime
            placeholder={['最近使用 开始', '最近使用 结束']}
            value={updatedAtRange}
            onChange={setUpdatedAtRange}
          />
          <Button
            type="primary"
            icon={<SearchOutlined />}
            onClick={handleSearch}
          >
            搜索
          </Button>
        </Space>

        <Table
          columns={columns}
          dataSource={users}
          rowKey="id"
          loading={loading}
          pagination={{
            current: page,
            pageSize,
            total,
            showSizeChanger: true,
            showTotal: (total) => `共 ${total} 条`,
            onChange: (page, pageSize) => {
              setPage(page);
              setPageSize(pageSize);
            },
          }}
        />
      </Card>
    </div>
  );
};

export default UserManagementPage;
