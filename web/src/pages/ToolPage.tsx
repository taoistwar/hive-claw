import { useEffect } from 'react';
import { Typography, Button, Table, Tag, Form, Input, Select, Space, DatePicker, Popconfirm } from 'antd';
import { PlusOutlined, EditOutlined, DeleteOutlined, SearchOutlined, ReloadOutlined, EyeOutlined } from '@ant-design/icons';
import type { ColumnsType } from 'antd/es/table';
import { useTool } from '../hooks/useTool';
import ToolForm from '../components/ToolForm';
import ToolDetail from '../components/ToolDetail';
import type { ToolItem, ToolSearchParams } from '../services/tool';

const { Title } = Typography;
const { RangePicker } = DatePicker;

const KIND_MAP: Record<number, { label: string; color: string }> = {
  1: { label: 'function-wrap', color: 'green' },
  2: { label: 'workflow-wrap', color: 'cyan' },
};

const ToolPage = () => {
  const {
    tools,
    loading,
    pagination,
    modalVisible,
    detailVisible,
    editingTool,
    viewingTool,
    searchParams,
    fetchTools,
    handleCreate,
    handleUpdate,
    handleDelete,
    openCreateModal,
    openEditModal,
    openDetailModal,
    closeModal,
    closeDetailModal,
    setPagination,
    setSearchParams,
    handleSearch,
    handleReset,
  } = useTool();

  const [form] = Form.useForm();

  useEffect(() => {
    fetchTools();
  }, []);

  const handlePaginationChange = (page: number, pageSize: number) => {
    setPagination((prev) => ({ ...prev, current: page, pageSize }));
  };

  const doSearch = () => {
    const values = form.getFieldsValue();
    const params: ToolSearchParams = {
      search: values.search || undefined,
      kind: values.kind,
    };

    if (values.created_at_range && values.created_at_range.length === 2) {
      params.created_at_start = values.created_at_range[0].format('YYYY-MM-DD');
      params.created_at_end = values.created_at_range[1].add(1, 'day').format('YYYY-MM-DD');
    }

    if (values.updated_at_range && values.updated_at_range.length === 2) {
      params.updated_at_start = values.updated_at_range[0].format('YYYY-MM-DD');
      params.updated_at_end = values.updated_at_range[1].add(1, 'day').format('YYYY-MM-DD');
    }

    setSearchParams(params);
    handleSearch(params);
  };

  const handleResetSearch = () => {
    form.resetFields();
    handleReset();
  };

  const columns: ColumnsType<ToolItem> = [
    {
      title: 'ID',
      dataIndex: 'id',
      width: 60,
    },
    {
      title: 'identifier',
      dataIndex: 'identifier',
      width: 150,
    },
    {
      title: '名称',
      dataIndex: 'name',
    },
    {
      title: '类型',
      dataIndex: 'kind',
      width: 140,
      render: (k: number) => {
        const kindInfo = KIND_MAP[k];
        return kindInfo ? <Tag color={kindInfo.color}>{kindInfo.label}</Tag> : k;
      },
    },
    {
      title: 'function_id',
      dataIndex: 'function_id',
      width: 100,
      render: (v: number | null) => v ?? '-',
    },
    {
      title: 'workflow_id',
      dataIndex: 'workflow_id',
      width: 100,
      render: (v: number | null) => v ?? '-',
    },
    {
      title: '创建时间',
      dataIndex: 'created_at',
      width: 180,
      render: (value: string) => new Date(value).toLocaleString(),
    },
    {
      title: '修改时间',
      dataIndex: 'updated_at',
      width: 180,
      render: (value: string) => new Date(value).toLocaleString(),
    },
    {
      title: '操作',
      key: 'action',
      width: 200,
      render: (_: unknown, record: ToolItem) => (
        <span>
          <Button
            type="link"
            size="small"
            icon={<EyeOutlined />}
            onClick={() => openDetailModal(record)}
          >
            查看
          </Button>
          <Button
            type="link"
            size="small"
            icon={<EditOutlined />}
            onClick={() => openEditModal(record)}
          >
            编辑
          </Button>
          <Popconfirm
            title="确认删除"
            description="确定要删除该工具吗？"
            onConfirm={() => handleDelete(record.id)}
            okText="确定"
            cancelText="取消"
          >
            <Button type="link" size="small" danger icon={<DeleteOutlined />}>
              删除
            </Button>
          </Popconfirm>
        </span>
      ),
    },
  ];

  return (
    <div>
      <div
        style={{
          display: 'flex',
          justifyContent: 'space-between',
          alignItems: 'center',
          marginBottom: 24,
        }}
      >
        <Title level={4} style={{ margin: 0 }}>
          工具管理
        </Title>
        <Button type="primary" icon={<PlusOutlined />} onClick={openCreateModal}>
          添加工具
        </Button>
      </div>

      <Form
        form={form}
        layout="inline"
        onFinish={doSearch}
        style={{ marginBottom: 16 }}
        aria-label="工具筛选"
      >
        <Form.Item name="search" label="搜索">
          <Input
            placeholder="搜索名称/标识符/描述"
            allowClear
            style={{ width: 200 }}
            aria-label="搜索名称、标识符或描述"
          />
        </Form.Item>
        <Form.Item name="kind" label="类型">
          <Select
            placeholder="类型"
            allowClear
            style={{ width: 140 }}
            aria-label="按类型筛选"
          >
            <Select.Option value={1}>function-wrap</Select.Option>
            <Select.Option value={2}>workflow-wrap</Select.Option>
          </Select>
        </Form.Item>
        <Form.Item name="created_at_range" label="创建时间">
          <RangePicker
            placeholder={['创建时间起', '创建时间止']}
            aria-label="按创建时间筛选"
          />
        </Form.Item>
        <Form.Item name="updated_at_range" label="修改时间">
          <RangePicker
            placeholder={['修改时间起', '修改时间止']}
            aria-label="按修改时间筛选"
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

      <Table<ToolItem>
        columns={columns}
        dataSource={tools}
        rowKey="id"
        loading={loading}
        pagination={{
          current: pagination.current,
          pageSize: pagination.pageSize,
          total: pagination.total,
          showSizeChanger: false,
          showTotal: (total) => `共 ${total} 条`,
          onChange: handlePaginationChange,
        }}
      />

      <ToolForm
        visible={modalVisible}
        editingTool={editingTool}
        onCancel={closeModal}
        onCreate={handleCreate}
        onUpdate={handleUpdate}
      />

      <ToolDetail
        visible={detailVisible}
        tool={viewingTool}
        onCancel={closeDetailModal}
      />
    </div>
  );
};

export default ToolPage;
