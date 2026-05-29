// FunctionPage — 列表 + 左侧分类树过滤 + 创建 + 编辑 + 删除 (T092 扩展)

import { useEffect, useState } from 'react';
import { Table, Tag, Space, Input, Select, message, Button, Popconfirm, Tree, Drawer, Form, Row, Col, DatePicker, Card } from 'antd';
import type { DataNode } from 'antd/es/tree';
import { PlusOutlined, EditOutlined, DeleteOutlined, PlayCircleOutlined, EyeOutlined, SearchOutlined, FilterOutlined, ReloadOutlined } from '@ant-design/icons';
import type { ColumnsType } from 'antd/es/table';
import type { Dayjs } from 'dayjs';
import { listFunctions, deleteFunction, type FunctionItem, type FunctionList } from '../services/function';
import FunctionForm from '../components/FunctionForm';
import FunctionTester from '../components/FunctionTester';
import { FunctionDetail } from '../components/FunctionDetail';
import { listCategoriesTree, type CategoryNode } from '../services/category';
import { listCapabilities, type CapabilityItem } from '../services/capability';

const { RangePicker } = DatePicker;

type FormMode = 'create' | 'edit' | null;

interface FilterValues {
  search?: string;
  kind?: 'builtin' | 'custom' | '';
  identifier?: string;
  name?: string;
  plugin_identifier?: string;
  required_capabilities?: string;
  created_at_range?: [Dayjs, Dayjs] | null;
  updated_at_range?: [Dayjs, Dayjs] | null;
}

function toDataNodes(nodes: CategoryNode[]): DataNode[] {
  return nodes.map((n) => ({
    key: n.id,
    title: n.name,
    children: n.children?.length ? toDataNodes(n.children) : undefined,
  }));
}

export default function FunctionPage() {
  const [data, setData] = useState<FunctionList>({ items: [], total: 0, offset: 0, limit: 20 });
  const [loading, setLoading] = useState(false);
  const [categoryId, setCategoryId] = useState<number | undefined>(undefined);
  const [formMode, setFormMode] = useState<FormMode>(null);
  const [editingRecord, setEditingRecord] = useState<FunctionItem | null>(null);
  const [testingRecord, setTestingRecord] = useState<FunctionItem | null>(null);
  const [testerOpen, setTesterOpen] = useState(false);
  const [viewOpen, setViewOpen] = useState(false);
  const [viewingRecord, setViewingRecord] = useState<FunctionItem | null>(null);
  const [categoryTree, setCategoryTree] = useState<CategoryNode[]>([]);
  const [selectedCategoryKey, setSelectedCategoryKey] = useState<React.Key | undefined>(undefined);
  const [treeExpandedKeys, setTreeExpandedKeys] = useState<React.Key[]>([]);
  const [treeLoading, setTreeLoading] = useState(false);
  const [filterForm] = Form.useForm<FilterValues>();
  const [filterCollapsed, setFilterCollapsed] = useState(false);
  const [capabilities, setCapabilities] = useState<CapabilityItem[]>([]);
  const [currentPage, setCurrentPage] = useState(1);
  const [pageSize, setPageSize] = useState(20);

  const loadCategoryTree = async () => {
    setTreeLoading(true);
    try {
      const [tree, caps] = await Promise.all([
        listCategoriesTree(),
        listCapabilities(),
      ]);
      setCategoryTree(tree);
      setCapabilities(caps);
      const allKeys: React.Key[] = [];
      const collect = (nodes: CategoryNode[]) => {
        nodes.forEach((n) => {
          allKeys.push(n.id);
          if (n.children?.length) collect(n.children);
        });
      };
      collect(tree);
      setTreeExpandedKeys(allKeys);
    } catch (e) {
      void message.error(`加载分类树失败：${(e as Error).message}`);
    } finally {
      setTreeLoading(false);
    }
  };

  useEffect(() => {
    void loadCategoryTree();
  }, []);

  const fetchData = (overrides?: Partial<FilterValues>) => {
    const values = overrides || filterForm.getFieldsValue();
    const offset = (currentPage - 1) * pageSize;
    setLoading(true);
    listFunctions({
      search: values.search || undefined,
      kind: values.kind || undefined,
      category_id: categoryId,
      identifier: values.identifier || undefined,
      name: values.name || undefined,
      plugin_identifier: values.plugin_identifier || undefined,
      required_capabilities: values.required_capabilities || undefined,
      created_at_start: values.created_at_range ? values.created_at_range[0].startOf('day').toISOString() : undefined,
      created_at_end: values.created_at_range ? values.created_at_range[1].endOf('day').toISOString() : undefined,
      updated_at_start: values.updated_at_range ? values.updated_at_range[0].startOf('day').toISOString() : undefined,
      updated_at_end: values.updated_at_range ? values.updated_at_range[1].endOf('day').toISOString() : undefined,
      offset,
      limit: pageSize,
    })
      .then(setData)
      .catch((e) => message.error(`加载失败：${(e as Error).message}`))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    fetchData();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [categoryId]);

  useEffect(() => {
    fetchData();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentPage, pageSize]);

  const handleFilter = () => {
    setCurrentPage(1);
    fetchData();
  };

  const handleReset = () => {
    filterForm.resetFields();
    setCurrentPage(1);
    fetchData({});
  };

  const handleCategorySelect = (key?: React.Key) => {
    const newSelected = key === selectedCategoryKey ? undefined : key;
    setSelectedCategoryKey(newSelected);
    setCategoryId(newSelected !== undefined ? Number(newSelected) : undefined);
    setCurrentPage(1);
  };

  const handleCreate = () => {
    setEditingRecord(null);
    setFormMode('create');
  };

  const handleEdit = (record: FunctionItem) => {
    setEditingRecord(record);
    setFormMode('edit');
  };

  const handleTest = (record: FunctionItem) => {
    setTestingRecord(record);
    setTesterOpen(true);
  };

  const handleView = (record: FunctionItem) => {
    setViewingRecord(record);
    setViewOpen(true);
  };

  const handleViewEdit = () => {
    setViewOpen(false);
    setViewingRecord(null);
    if (viewingRecord) {
      handleEdit(viewingRecord);
    }
  };

  const handleViewClose = () => {
    setViewOpen(false);
    setViewingRecord(null);
  };

  const handleDelete = async (record: FunctionItem) => {
    try {
      await deleteFunction(record.id);
      message.success('函数删除成功');
      fetchData();
    } catch (e) {
      message.error(`删除失败：${(e as Error).message}`);
    }
  };

  const handleFormSuccess = () => {
    fetchData();
  };

  const handleFormClose = () => {
    setFormMode(null);
    setEditingRecord(null);
  };

  const columns: ColumnsType<FunctionItem> = [
    { title: 'ID', dataIndex: 'id', width: 60 },
    { title: 'identifier', dataIndex: 'identifier' },
    { title: 'name', dataIndex: 'name' },
    {
      title: 'kind',
      dataIndex: 'kind',
      width: 100,
      render: (k: number) =>
        k === 1 ? <Tag color="purple">builtin</Tag> : <Tag color="blue">custom</Tag>,
    },
    { title: 'plugin', dataIndex: 'plugin_identifier', render: (v?: string | null) => v ?? '-' },
    { title: 'plugin_export', dataIndex: 'plugin_export' },
    {
      title: 'required_capabilities',
      dataIndex: 'required_capabilities',
      render: (caps?: string[] | null) =>
        caps?.map((c) => <Tag key={c}>{c}</Tag>) ?? '-',
    },
    {
      title: 'tags',
      dataIndex: 'tags',
      render: (tags?: { id: number; name: string }[]) =>
        tags?.map((t) => <Tag key={t.id}>{t.name}</Tag>) ?? null,
    },
    { title: '创建时间', dataIndex: 'created_at', width: 180, render: (v: string) => new Date(v).toLocaleString() },
    { title: '更新时间', dataIndex: 'updated_at', width: 180, render: (v: string) => new Date(v).toLocaleString() },
    {
      title: '操作',
      width: 260,
      render: (_: unknown, record: FunctionItem) => (
        <Space>
          <Button
            type="link"
            size="small"
            icon={<EyeOutlined />}
            onClick={() => handleView(record)}
          >
            查看
          </Button>
          <Button
            type="link"
            size="small"
            icon={<PlayCircleOutlined />}
            onClick={() => handleTest(record)}
          >
            测试
          </Button>
          <Button
            type="link"
            size="small"
            icon={<EditOutlined />}
            onClick={() => handleEdit(record)}
          >
            编辑
          </Button>
          {record.kind !== 1 && (
            <Popconfirm
              title="确认删除"
              description="确定要删除此函数吗？此操作不可撤销。"
              onConfirm={() => handleDelete(record)}
              okText="确认"
              cancelText="取消"
            >
              <Button
                type="link"
                size="small"
                danger
                icon={<DeleteOutlined />}
              >
                删除
              </Button>
            </Popconfirm>
          )}
        </Space>
      ),
    },
  ];

  return (
    <div style={{ display: 'flex', gap: 16 }}>
      <div style={{ width: 280, flexShrink: 0, borderRight: '1px solid #f0f0f0', paddingRight: 16 }}>
        <h4 style={{ margin: '0 0 8px 0' }}>分类</h4>
        <Button size="small" style={{ width: '100%', marginBottom: 8 }} onClick={() => handleCategorySelect()}>
          全部
        </Button>
        {treeLoading ? <p>加载中…</p> : (
          <Tree
            treeData={toDataNodes(categoryTree)}
            expandedKeys={treeExpandedKeys}
            onExpand={(keys) => setTreeExpandedKeys(keys)}
            selectedKeys={selectedCategoryKey !== undefined ? [selectedCategoryKey] : []}
            onSelect={(keys) => {
              if (keys.length > 0) handleCategorySelect(keys[0]);
              else handleCategorySelect();
            }}
            showLine
          />
        )}
      </div>
      <div style={{ flex: 1, minWidth: 0 }}>
        <Space style={{ marginBottom: 16 }}>
          <Button
            type="primary"
            icon={<PlusOutlined />}
            onClick={handleCreate}
          >
            创建函数
          </Button>
          <Button
            icon={filterCollapsed ? <FilterOutlined /> : <SearchOutlined />}
            onClick={() => setFilterCollapsed(!filterCollapsed)}
          >
            {filterCollapsed ? '展开筛选' : '收起筛选'}
          </Button>
        </Space>

        {!filterCollapsed && (
          <Card size="small" style={{ marginBottom: 16 }}>
            <Form form={filterForm} layout="inline">
              <Row gutter={[16, 8]} style={{ width: '100%' }}>
                <Col span={6}>
                  <Form.Item name="search" label="关键词" style={{ width: '100%', marginBottom: 0 }}>
                    <Input placeholder="搜索名称/描述/identifier" allowClear />
                  </Form.Item>
                </Col>
                <Col span={6}>
                  <Form.Item name="kind" label="类型" style={{ width: '100%', marginBottom: 0 }}>
                    <Select
                      placeholder="选择类型"
                      allowClear
                      options={[
                        { value: 'builtin', label: 'builtin' },
                        { value: 'custom', label: 'custom' },
                      ]}
                    />
                  </Form.Item>
                </Col>
                <Col span={6}>
                  <Form.Item name="identifier" label="identifier" style={{ width: '100%', marginBottom: 0 }}>
                    <Input placeholder="精确匹配" allowClear />
                  </Form.Item>
                </Col>
                <Col span={6}>
                  <Form.Item name="name" label="名称" style={{ width: '100%', marginBottom: 0 }}>
                    <Input placeholder="模糊匹配" allowClear />
                  </Form.Item>
                </Col>
                <Col span={6}>
                  <Form.Item name="plugin_identifier" label="插件" style={{ width: '100%', marginBottom: 0 }}>
                    <Input placeholder="模糊匹配" allowClear />
                  </Form.Item>
                </Col>
                <Col span={6}>
                  <Form.Item name="required_capabilities" label="权限" style={{ width: '100%', marginBottom: 0 }}>
                    <Select
                      placeholder="选择capability"
                      allowClear
                      options={capabilities.map((c) => ({
                        label: (
                          <span>
                            {c.name}
                            {c.is_dangerous && <Tag color="red" style={{ marginLeft: 4 }}>危险</Tag>}
                          </span>
                        ),
                        value: c.name,
                      }))}
                      showSearch
                      filterOption={(input: string, option) => {
                        const cap = capabilities.find((c) => c.name === (option as any)?.value);
                        if (!cap) return false;
                        return cap.name.toLowerCase().includes(input.toLowerCase());
                      }}
                    />
                  </Form.Item>
                </Col>
                <Col span={6}>
                  <Form.Item label="创建时间" style={{ width: '100%', marginBottom: 0 }}>
                    <Form.Item name="created_at_range" noStyle>
                      <RangePicker style={{ width: '100%' }} />
                    </Form.Item>
                  </Form.Item>
                </Col>
                <Col span={6}>
                  <Form.Item label="更新时间" style={{ width: '100%', marginBottom: 0 }}>
                    <Form.Item name="updated_at_range" noStyle>
                      <RangePicker style={{ width: '100%' }} />
                    </Form.Item>
                  </Form.Item>
                </Col>
                <Col span={6}>
                  <Form.Item style={{ marginBottom: 0 }}>
                    <Button type="primary" icon={<SearchOutlined />} onClick={handleFilter}>
                      查询
                    </Button>
                    <Button style={{ marginLeft: 8 }} icon={<ReloadOutlined />} onClick={handleReset}>
                      重置
                    </Button>
                  </Form.Item>
                </Col>
              </Row>
            </Form>
          </Card>
        )}

        <Table<FunctionItem>
          rowKey="id"
          columns={columns}
          dataSource={data.items}
          loading={loading}
          pagination={{ 
            total: data.total, 
            current: currentPage,
            pageSize: pageSize,
            showSizeChanger: true,
            pageSizeOptions: ['10', '20', '50', '100'],
            showTotal: (total) => `共 ${total} 条`,
          }}
          onChange={(pagination) => {
            setCurrentPage(pagination.current || 1);
            setPageSize(pagination.pageSize || 20);
          }}
        />

        <FunctionForm
          open={formMode !== null}
          mode={formMode === 'create' ? 'create' : 'edit'}
          record={editingRecord}
          onClose={handleFormClose}
          onSuccess={handleFormSuccess}
        />

        {testingRecord && (
          <FunctionTester
            functionItem={testingRecord}
            open={testerOpen}
            onClose={() => {
              setTesterOpen(false);
              setTestingRecord(null);
            }}
          />
        )}

        <Drawer
          title="查看函数"
          open={viewOpen}
          onClose={handleViewClose}
          width={720}
        >
          {viewingRecord && (
            <FunctionDetail
              fn={viewingRecord}
              onEdit={handleViewEdit}
              onBack={handleViewClose}
            />
          )}
        </Drawer>
      </div>
    </div>
  );
}
