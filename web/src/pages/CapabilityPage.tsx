import { useCallback, useEffect, useMemo, useState } from 'react';
import { Button, Card, Col, Drawer, Form, Input, Row, Select, Space, Table, Tag, Tree, message, Modal, Popconfirm, Tooltip, Dropdown } from 'antd';
import type { MenuProps } from 'antd';
import { FilterOutlined, ReloadOutlined, SearchOutlined, PlusOutlined, EditOutlined, DeleteOutlined, EyeOutlined } from '@ant-design/icons';
import type { ColumnsType } from 'antd/es/table';
import type { DataNode } from 'antd/es/tree';
import {
  listCapabilities,
  createCapability,
  updateCapability,
  deleteCapability,
  type CapabilityItem,
  type CapabilityDetail,
  getCapabilityDetail,
} from '../services/capability';
import { listCategoriesTree, type CategoryNode } from '../services/category';

interface FilterValues {
  name?: string;
  description?: string;
  is_dangerous?: boolean;
}

function toDataNodes(nodes: CategoryNode[]): DataNode[] {
  return nodes.map((n) => ({
    key: n.id,
    title: n.name,
    children: n.children?.length ? toDataNodes(n.children) : undefined,
  }));
}

export default function CapabilityPage() {
  const [items, setItems] = useState<CapabilityItem[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [viewOpen, setViewOpen] = useState(false);
  const [viewingCapability, setViewingCapability] = useState<CapabilityDetail | null>(null);
  const [viewLoading, setViewLoading] = useState(false);
  const [filterCollapsed, setFilterCollapsed] = useState(false);
  const [filterForm] = Form.useForm();
  const [selectedCategoryKey, setSelectedCategoryKey] = useState<React.Key | undefined>(undefined);
  const [treeExpandedKeys, setTreeExpandedKeys] = useState<React.Key[]>([]);
  const [categoryTree, setCategoryTree] = useState<CategoryNode[]>([]);
  const [treeLoading, setTreeLoading] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [createForm] = Form.useForm();
  const [editOpen, setEditOpen] = useState(false);
  const [editForm] = Form.useForm();
  const [editingCap, setEditingCap] = useState<CapabilityItem | null>(null);
  const [currentPage, setCurrentPage] = useState(1);
  const [pageSize, setPageSize] = useState(20);

  const loadCategoryTree = async () => {
    setTreeLoading(true);
    try {
      const tree = await listCategoriesTree();
      setCategoryTree(tree);
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

  const fetchData = useCallback(() => {
    const values = filterForm.getFieldsValue() as FilterValues;
    const offset = (currentPage - 1) * pageSize;
    setLoading(true);
    listCapabilities({
      name: values.name || undefined,
      description: values.description || undefined,
      is_dangerous: values.is_dangerous,
      category_id: selectedCategoryKey !== undefined ? Number(selectedCategoryKey) : undefined,
      offset,
      limit: pageSize,
    })
      .then(([data, t]) => { setItems(data); setTotal(t); })
      .catch((e) => message.error(`加载失败：${(e as Error).message}`))
      .finally(() => setLoading(false));
  }, [currentPage, pageSize, selectedCategoryKey, filterForm]);

  useEffect(() => {
    fetchData();
  }, [fetchData]);

  const handleFilter = () => {
    setCurrentPage(1);
    fetchData();
  };

  const handleReset = () => {
    filterForm.resetFields();
    setSelectedCategoryKey(undefined);
    setCurrentPage(1);
    fetchData();
  };

  const handleCategorySelect = (key?: React.Key) => {
    const newSelected = key === selectedCategoryKey ? undefined : key;
    setSelectedCategoryKey(newSelected);
    setCurrentPage(1);
  };

  const handleView = async (cap: CapabilityItem) => {
    setViewLoading(true);
    setViewingCapability(null);
    setViewOpen(true);
    try {
      const detail = await getCapabilityDetail(cap.name);
      setViewingCapability(detail);
    } catch (e) {
      void message.error(`加载详情失败：${(e as Error).message}`);
    } finally {
      setViewLoading(false);
    }
  };

  const onCreate = async (values: { name: string; description: string; is_dangerous: boolean; category_id?: number }) => {
    try {
      await createCapability(values);
      void message.success('已创建');
      setCreateOpen(false);
      createForm.resetFields();
      fetchData();
    } catch (e) {
      void message.error(`创建失败：${(e as Error).message}`);
    }
  };

  const openEdit = (cap: CapabilityItem) => {
    setEditingCap(cap);
    editForm.setFieldsValue({
      description: cap.description,
      is_dangerous: cap.is_dangerous,
      category_id: cap.category_id,
    });
    setEditOpen(true);
  };

  const onEdit = async (values: { description?: string; is_dangerous?: boolean; category_id?: number | null }) => {
    if (!editingCap) return;
    try {
      await updateCapability(editingCap.name, values);
      void message.success('更新成功');
      editForm.resetFields();
      setEditOpen(false);
      setEditingCap(null);
      fetchData();
    } catch (e) {
      void message.error(`更新失败：${(e as Error).message}`);
    }
  };

  const onDelete = async (cap: CapabilityItem) => {
    try {
      await deleteCapability(cap.name);
      void message.success('已删除');
      fetchData();
    } catch (e) {
      void message.error(`删除失败：${(e as Error).message}`);
    }
  };

  const columns: ColumnsType<CapabilityItem> = [
    {
      title: '名称',
      dataIndex: 'name',
      width: 200,
      render: (name: string) => <Tag color="blue">{name}</Tag>,
    },
    { title: '描述', dataIndex: 'description' },
    {
      title: '危险性',
      dataIndex: 'is_dangerous',
      width: 100,
      render: (v: boolean) =>
        v ? <Tag color="red">危险</Tag> : <Tag color="green">安全</Tag>,
    },
    {
      title: '操作',
      key: 'actions',
      width: 80,
      render: (_, r) => {
        const items: MenuProps['items'] = [
          {
            key: 'view',
            label: '查看',
            icon: <EyeOutlined />,
            onClick: () => void handleView(r),
          },
          {
            key: 'edit',
            label: '编辑',
            icon: <EditOutlined />,
            onClick: () => openEdit(r),
          },
          { type: 'divider' },
          {
            key: 'delete',
            label: (
              <Popconfirm
                title="确认删除"
                description={`确定要删除 capability「${r.name}」吗？`}
                onConfirm={() => void onDelete(r)}
                okText="确认"
                cancelText="取消"
              >
                <span style={{ color: '#ff4d4f' }}>删除</span>
              </Popconfirm>
            ),
            icon: <DeleteOutlined style={{ color: '#ff4d4f' }} />,
            danger: true,
          },
        ];

        return (
          <Space size="small">
            <Tooltip title="查看">
              <Button
                type="text"
                size="small"
                icon={<EyeOutlined />}
                onClick={() => void handleView(r)}
              />
            </Tooltip>
            <Dropdown menu={{ items }} placement="bottomRight" trigger={['click']}>
              <Tooltip title="更多操作">
                <Button type="text" size="small" icon={<DeleteOutlined />} />
              </Tooltip>
            </Dropdown>
          </Space>
        );
      },
    },
  ];

  const treeData = useMemo(() => toDataNodes(categoryTree), [categoryTree]);

  return (
    <div style={{ display: 'flex', height: 'calc(100vh - 110px)' }}>
      <Card
        title="分类"
        size="small"
        style={{ width: 220, flexShrink: 0, marginRight: 16, overflowY: 'auto' }}
        extra={
          <Button
            type="text"
            size="small"
            icon={<ReloadOutlined />}
            onClick={() => void loadCategoryTree()}
            loading={treeLoading}
          />
        }
      >
        <Space direction="vertical" style={{ width: '100%' }}>
          <Button
            type={!selectedCategoryKey ? 'primary' : 'default'}
            block
            size="small"
            onClick={() => {
              setSelectedCategoryKey(undefined);
              setCurrentPage(1);
            }}
          >
            全部
          </Button>
          <Tree
            treeData={treeData}
            expandedKeys={treeExpandedKeys}
            onExpand={(keys) => setTreeExpandedKeys(keys)}
            selectedKeys={selectedCategoryKey ? [selectedCategoryKey] : []}
            onSelect={([key]) => handleCategorySelect(key)}
            blockNode
          />
        </Space>
      </Card>

      <div style={{ flex: 1, overflow: 'auto' }}>
        <Card
          style={{ marginBottom: 16 }}
          title="搜索过滤"
          size="small"
          extra={
            <Button type="primary" icon={<PlusOutlined />} onClick={() => setCreateOpen(true)}>
              新建 Capability
            </Button>
          }
        >
          <Form
            form={filterForm}
            onFinish={handleFilter}
          >
            <Row gutter={[16, 12]}>
              <Col span={6}>
                <Form.Item name="name" label="名称" style={{ marginBottom: 0 }}>
                  <Input placeholder="模糊匹配" allowClear />
                </Form.Item>
              </Col>
              <Col span={6}>
                <Form.Item name="description" label="描述" style={{ marginBottom: 0 }}>
                  <Input placeholder="模糊匹配" allowClear />
                </Form.Item>
              </Col>
              <Col span={6}>
                <Form.Item name="is_dangerous" label="危险性" style={{ marginBottom: 0 }}>
                  <Select placeholder="请选择" allowClear>
                    <Select.Option value={true}>危险</Select.Option>
                    <Select.Option value={false}>安全</Select.Option>
                  </Select>
                </Form.Item>
              </Col>
              <Col span={6}>
                <Form.Item style={{ marginBottom: 0 }}>
                  <Button type="primary" icon={<SearchOutlined />} htmlType="submit">
                    查询
                  </Button>
                  <Button onClick={handleReset} icon={<ReloadOutlined />} style={{ marginLeft: 8 }}>
                    重置
                  </Button>
                </Form.Item>
              </Col>
            </Row>
          </Form>
        </Card>

        <Table<CapabilityItem>
          rowKey="name"
          columns={columns}
          dataSource={items}
          loading={loading}
          pagination={{
            current: currentPage,
            pageSize: pageSize,
            total: total,
            showTotal: (t) => `共 ${t} 条`,
            showSizeChanger: true,
            pageSizeOptions: ['10', '20', '50', '100'],
            onChange: (page, pageSize) => {
              setCurrentPage(page);
              setPageSize(pageSize);
            },
          }}
        />
      </div>

      <Drawer
        title={viewingCapability ? `Capability「${viewingCapability.name}」` : '查看 Capability'}
        open={viewOpen}
        width={600}
        onClose={() => {
          setViewOpen(false);
          setViewingCapability(null);
        }}
      >
        {viewLoading && <p>加载中…</p>}
        {viewingCapability && (
          <Space direction="vertical" style={{ width: '100%' }} size="large">
            <div>
              <strong>名称：</strong>
              <Tag color="blue">{viewingCapability.name}</Tag>
            </div>
            <div>
              <strong>描述：</strong>
              <p style={{ marginTop: 4 }}>{viewingCapability.description}</p>
            </div>
            <div>
              <strong>危险性：</strong>
              {viewingCapability.is_dangerous ? (
                <Tag color="red">危险</Tag>
              ) : (
                <Tag color="green">安全</Tag>
              )}
            </div>
            {viewingCapability.allowed_plugins !== undefined && (
              <div>
                <strong>允许使用的插件：</strong>
                <p style={{ marginTop: 4 }}>
                  {viewingCapability.allowed_plugins.length > 0
                    ? viewingCapability.allowed_plugins.join(', ')
                    : '无'}
                </p>
              </div>
            )}
          </Space>
        )}
      </Drawer>

      <Modal
        title="新建 Capability"
        open={createOpen}
        onCancel={() => setCreateOpen(false)}
        onOk={() => createForm.submit()}
        destroyOnHidden
        width={560}
      >
        <Form form={createForm} layout="vertical" onFinish={onCreate}>
          <Form.Item name="name" label="名称" rules={[{ required: true }]}>
            <Input placeholder="network.http" />
          </Form.Item>
          <Form.Item name="description" label="描述" rules={[{ required: true }]}>
            <Input.TextArea rows={2} />
          </Form.Item>
          <Form.Item name="is_dangerous" label="危险性" rules={[{ required: true }]}>
            <Select>
              <Select.Option value={false}>安全</Select.Option>
              <Select.Option value={true}>危险</Select.Option>
            </Select>
          </Form.Item>
          <Form.Item name="category_id" label="分类">
            <Select
              placeholder="选择分类"
              allowClear
              options={categoryTree.map((c) => ({ value: c.id, label: c.name }))}
            />
          </Form.Item>
        </Form>
      </Modal>

      <Modal
        title="编辑 Capability"
        open={editOpen}
        onCancel={() => {
          setEditOpen(false);
          setEditingCap(null);
        }}
        onOk={() => editForm.submit()}
        destroyOnHidden
        width={560}
      >
        <Form form={editForm} layout="vertical" onFinish={onEdit}>
          <Form.Item name="description" label="描述">
            <Input.TextArea rows={2} />
          </Form.Item>
          <Form.Item name="is_dangerous" label="危险性">
            <Select>
              <Select.Option value={false}>安全</Select.Option>
              <Select.Option value={true}>危险</Select.Option>
            </Select>
          </Form.Item>
          <Form.Item name="category_id" label="分类">
            <Select
              placeholder="选择分类"
              allowClear
              options={categoryTree.map((c) => ({ value: c.id, label: c.name }))}
            />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
