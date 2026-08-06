// WorkflowPage — 列表 + 左侧分类树过滤 + 标签 + 创建 + 编辑 + 删除

import { useEffect, useState } from 'react';
import { Table, Tag, Space, Input, InputNumber, message, Button, Popconfirm, Tree, Drawer, Form, Row, Col, DatePicker, Card, Select, Modal, Tooltip, Dropdown } from 'antd';
import type { DataNode } from 'antd/es/tree';
import type { ColumnsType } from 'antd/es/table';
import type { Dayjs } from 'dayjs';
import type { MenuProps } from 'antd';
import { PlusOutlined, EditOutlined, DeleteOutlined, PlayCircleOutlined, EyeOutlined, SearchOutlined, FilterOutlined, ReloadOutlined, SettingOutlined, UnorderedListOutlined, EllipsisOutlined } from '@ant-design/icons';
import dayjs from 'dayjs';

import { DagEditor } from '../components/DagEditor/DagEditor';
import { CategoryTreeSelect } from '../components/CategoryTreeSelect';
import {
  createWorkflow,
  deleteWorkflow,
  listWorkflows,
  updateWorkflow,
  type WorkflowMeta,
} from '../services/workflow';
import { listCategoriesTree, type CategoryNode } from '../services/category';
import { listTags, type TagItem } from '../services/tag';
import { listCapabilities, type CapabilityItem } from '../services/capability';

const { RangePicker } = DatePicker;

interface FilterValues {
  id?: number;
  identifier?: string;
  name?: string;
  search?: string;
  required_capabilities?: string;
  timeout_ms_from?: number;
  timeout_ms_to?: number;
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

function formatDate(v: string | undefined): string {
  if (!v) return '—';
  return dayjs(v).format('YYYY-MM-DD HH:mm');
}

function truncateDescription(v: string | null | undefined): string {
  if (!v) return '—';
  if (v.length <= 40) return v;
  return `${v.slice(0, 40)}…`;
}

export default function WorkflowPage() {
  const [data, setData] = useState<WorkflowMeta[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [categoryId, setCategoryId] = useState<number | undefined>(undefined);
  const [tagId, setTagId] = useState<number | undefined>(undefined);
  const [selected, setSelected] = useState<WorkflowMeta | null>(null);
  const [viewDagOpen, setViewDagOpen] = useState(false);
  const [viewingDagWf, setViewingDagWf] = useState<WorkflowMeta | null>(null);
  const [viewOpen, setViewOpen] = useState(false);
  const [viewingWf, setViewingWf] = useState<WorkflowMeta | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [createForm] = Form.useForm();
  const [editOpen, setEditOpen] = useState(false);
  const [editForm] = Form.useForm();
  const [editingWf, setEditingWf] = useState<WorkflowMeta | null>(null);
  const [categoryTree, setCategoryTree] = useState<CategoryNode[]>([]);
  const [selectedCategoryKey, setSelectedCategoryKey] = useState<React.Key | undefined>(undefined);
  const [treeExpandedKeys, setTreeExpandedKeys] = useState<React.Key[]>([]);
  const [treeLoading, setTreeLoading] = useState(false);
  const [filterForm] = Form.useForm<FilterValues>();
  const [allTags, setAllTags] = useState<TagItem[]>([]);
  const [capabilities, setCapabilities] = useState<CapabilityItem[]>([]);
  const [currentPage, setCurrentPage] = useState(1);
  const [pageSize, setPageSize] = useState(5);

  const loadCategoryTree = async () => {
    setTreeLoading(true);
    try {
      const [tree, tags, caps] = await Promise.all([
        listCategoriesTree(),
        listTags(),
        listCapabilities(),
      ]);
      setCategoryTree(tree);
      setAllTags(tags.items);
      setCapabilities(caps[0]);
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

  const fetchData = () => {
    const values = filterForm.getFieldsValue();
    const offset = (currentPage - 1) * pageSize;
    setLoading(true);
    listWorkflows({
      id: values.id,
      identifier: values.identifier || undefined,
      name: values.name || undefined,
      search: values.search || undefined,
      category_id: categoryId,
      tag_id: tagId,
      required_capabilities: values.required_capabilities || undefined,
      timeout_ms_from: values.timeout_ms_from,
      timeout_ms_to: values.timeout_ms_to,
      created_at_from: values.created_at_range ? values.created_at_range[0].startOf('day').toISOString() : undefined,
      created_at_to: values.created_at_range ? values.created_at_range[1].endOf('day').toISOString() : undefined,
      updated_at_from: values.updated_at_range ? values.updated_at_range[0].startOf('day').toISOString() : undefined,
      updated_at_to: values.updated_at_range ? values.updated_at_range[1].endOf('day').toISOString() : undefined,
      offset,
      limit: pageSize,
    })
      .then((resp) => { setData(resp.items); setTotal(resp.total); })
      .catch((e) => message.error(`加载失败：${(e as Error).message}`))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    fetchData();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [categoryId, tagId]);

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
    fetchData();
  };

  const handleCategorySelect = (key?: React.Key) => {
    const newSelected = key === selectedCategoryKey ? undefined : key;
    setSelectedCategoryKey(newSelected);
    setCategoryId(newSelected !== undefined ? Number(newSelected) : undefined);
    setTagId(undefined);
    setCurrentPage(1);
  };

  const onCreate = async (values: {
    identifier: string;
    name: string;
    description?: string;
    timeout_ms?: number;
    category_id?: number;
    required_capabilities?: string[];
    tag_ids?: number[];
  }) => {
    try {
      const wf = await createWorkflow(values);
      void message.success(`已创建 workflow #${wf.id}`);
      setCreateOpen(false);
      createForm.resetFields();
      fetchData();
    } catch (e) {
      void message.error(`创建失败：${(e as Error).message}`);
    }
  };

  const openEdit = (wf: WorkflowMeta) => {
    setEditingWf(wf);
    editForm.setFieldsValue({
      name: wf.name,
      description: wf.description,
      timeout_ms: wf.timeout_ms,
      category_id: wf.category_id,
      required_capabilities: wf.required_capabilities || [],
      tag_ids: wf.tags?.map((t) => t.id),
    });
    setEditOpen(true);
  };

  const onEdit = async (values: {
    name: string;
    description?: string;
    timeout_ms?: number;
    category_id?: number;
    required_capabilities?: string[];
    tag_ids?: number[];
  }) => {
    if (!editingWf) return;
    try {
      await updateWorkflow(editingWf.id, {
        ...values,
        updated_at: editingWf.updated_at,
      });
      void message.success('更新成功');
      editForm.resetFields();
      setEditOpen(false);
      setEditingWf(null);
      fetchData();
    } catch (e) {
      void message.error(`更新失败：${(e as Error).message}`);
    }
  };

  const onDelete = (wf: WorkflowMeta) => {
    Modal.confirm({
      title: `删除 workflow「${wf.identifier}」？`,
      content: '若被 Tool (kind=2) 引用，将返回 4093 阻止',
      okButtonProps: { danger: true },
      onOk: async () => {
        try {
          await deleteWorkflow(wf.id);
          void message.success('已删除');
          if (selected?.id === wf.id) setSelected(null);
          fetchData();
        } catch (e: unknown) {
          const err = e as { response?: { data?: { code?: number; message?: string } } };
          void message.error(err.response?.data?.message ?? (e as Error).message);
        }
      },
    });
  };

  const columns: ColumnsType<WorkflowMeta> = [
    { title: 'ID', dataIndex: 'id', width: 60 },
    { title: 'identifier', dataIndex: 'identifier' },
    { title: 'name', dataIndex: 'name' },
    {
      title: 'description',
      dataIndex: 'description',
      ellipsis: true,
      width: 160,
      render: (v: string | null) => (
        <Tooltip title={v || undefined}>
          {truncateDescription(v)}
        </Tooltip>
      ),
    },
    {
      title: 'required_capabilities',
      dataIndex: 'required_capabilities',
      width: 180,
      render: (caps?: string[] | null) =>
        caps?.map((c) => <Tag key={c}>{c}</Tag>) ?? '-',
    },
    {
      title: 'tags',
      dataIndex: 'tags',
      render: (tags?: { id: number; name: string }[]) =>
        tags?.map((t) => <Tag key={t.id}>{t.name}</Tag>) ?? '-',
    },
    { title: 'timeout_ms', dataIndex: 'timeout_ms', width: 110 },
    {
      title: 'created_at',
      dataIndex: 'created_at',
      width: 140,
      render: (v: string) => formatDate(v),
    },
    {
      title: 'updated_at',
      dataIndex: 'updated_at',
      width: 140,
      render: (v: string) => formatDate(v),
    },
    {
      title: '操作',
      key: 'actions',
      width: 80,
      render: (_, r) => {
        const items: MenuProps['items'] = [
          {
            key: 'view-dag',
            label: '查看 DAG',
            icon: <UnorderedListOutlined />,
            onClick: () => { setViewingDagWf(r); setViewDagOpen(true); },
          },
          {
            key: 'edit-dag',
            label: '编辑 DAG',
            icon: <SettingOutlined />,
            onClick: () => setSelected(r),
          },
          {
            key: 'edit-info',
            label: '编辑信息',
            icon: <EditOutlined />,
            onClick: () => openEdit(r),
          },
          { type: 'divider' },
          {
            key: 'delete',
            label: (
              <Popconfirm
                title="确认删除"
                description={`确定要删除 workflow「${r.identifier}」吗？若被 Tool 引用将阻止删除。`}
                onConfirm={() => onDelete(r)}
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
                onClick={() => { setViewingWf(r); setViewOpen(true); }}
              />
            </Tooltip>
            <Dropdown menu={{ items }} placement="bottomRight" trigger={['click']}>
              <Tooltip title="更多操作">
                <Button type="text" size="small" icon={<EllipsisOutlined />} />
              </Tooltip>
            </Dropdown>
          </Space>
        );
      },
    },
  ];

  const treeData = toDataNodes(categoryTree);

  return (
    <div style={{ display: 'flex', height: 'calc(100vh - 110px)' }}>
      {/* Left sidebar: category tree */}
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
              setCategoryId(undefined);
              setTagId(undefined);
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

      {/* Main content */}
      <div style={{ flex: 1, overflow: 'auto' }}>
        <Card
          style={{ marginBottom: 16 }}
          title="搜索过滤"
          size="small"
          extra={
            <Button type="primary" icon={<PlusOutlined />} onClick={() => setCreateOpen(true)}>
              新建 Workflow
            </Button>
          }
        >
          <Form
            form={filterForm}
            onFinish={handleFilter}
          >
            <Row gutter={[16, 12]}>
              <Col span={3}>
                <Form.Item name="id" label="ID" style={{ marginBottom: 0 }}>
                  <InputNumber placeholder="ID" style={{ width: '100%' }} />
                </Form.Item>
              </Col>
              <Col span={3}>
                <Form.Item name="identifier" label="identifier" style={{ marginBottom: 0 }}>
                  <Input placeholder="identifier" allowClear style={{ width: '100%' }} />
                </Form.Item>
              </Col>
              <Col span={3}>
                <Form.Item name="name" label="name" style={{ marginBottom: 0 }}>
                  <Input placeholder="name" allowClear style={{ width: '100%' }} />
                </Form.Item>
              </Col>
              <Col span={3}>
                <Form.Item name="required_capabilities" label="capabilities" style={{ marginBottom: 0 }}>
                  <Input placeholder="capability name" allowClear style={{ width: '100%' }} />
                </Form.Item>
              </Col>
              <Col span={6}>
                <Form.Item name="search" label="关键词" style={{ marginBottom: 0 }}>
                  <Input placeholder="identifier / name / description / capabilities" allowClear prefix={<SearchOutlined />} />
                </Form.Item>
              </Col>
              <Col span={6}>
                <Form.Item label="timeout_ms" style={{ marginBottom: 0 }}>
                  <Space.Compact style={{ width: '100%' }}>
                    <Form.Item name="timeout_ms_from" noStyle>
                      <InputNumber placeholder="min" min={0} style={{ width: '50%' }} />
                    </Form.Item>
                    <Form.Item name="timeout_ms_to" noStyle>
                      <InputNumber placeholder="max" min={0} style={{ width: '50%' }} />
                    </Form.Item>
                  </Space.Compact>
                </Form.Item>
              </Col>
              <Col span={6}>
                <Form.Item label="创建时间" style={{ marginBottom: 0 }}>
                  <RangePicker showTime style={{ width: '100%' }} />
                </Form.Item>
              </Col>
              <Col span={6}>
                <Form.Item label="更新时间" style={{ marginBottom: 0 }}>
                  <RangePicker showTime style={{ width: '100%' }} />
                </Form.Item>
              </Col>
              <Col span={6}>
                <Form.Item style={{ marginBottom: 0 }}>
                  <Button type="primary" icon={<SearchOutlined />} htmlType="submit">
                    搜索
                  </Button>
                  <Button onClick={handleReset} icon={<ReloadOutlined />} style={{ marginLeft: 8 }}>
                    重置
                  </Button>
                </Form.Item>
              </Col>
            </Row>
          </Form>
        </Card>

        <Table<WorkflowMeta>
          rowKey="id"
          columns={columns}
          dataSource={data}
          loading={loading}
          pagination={{
            current: currentPage,
            pageSize: pageSize,
            total: total,
            showTotal: (t) => `共 ${t} 条`,
            showSizeChanger: true,
            pageSizeOptions: ['5', '10', '20', '50', '100'],
            onChange: (page, pageSize) => {
              setCurrentPage(page);
              setPageSize(pageSize);
            },
          }}
        />

        {/* DAG Editor Drawer */}
        <Drawer
          title={selected ? `DAG 编辑器 — ${selected.identifier}` : ''}
          open={!!selected}
          width="80%"
          onClose={() => setSelected(null)}
          destroyOnHidden
        >
          {selected ? (
            <DagEditor workflowId={selected.id} onSaved={() => void fetchData()} />
          ) : null}
        </Drawer>

        {/* DAG Preview Drawer */}
        <Drawer
          title={viewingDagWf ? `DAG 预览 — ${viewingDagWf.identifier}` : ''}
          open={viewDagOpen}
          width="80%"
          onClose={() => { setViewDagOpen(false); setViewingDagWf(null); }}
          destroyOnHidden
        >
          {viewingDagWf ? (
            <DagEditor workflowId={viewingDagWf.id} readonly onSaved={() => void fetchData()} />
          ) : null}
        </Drawer>

        {/* View Info Drawer */}
        <Drawer
          title={viewingWf ? `Workflow 信息 — ${viewingWf.identifier}` : ''}
          open={viewOpen}
          width={500}
          onClose={() => { setViewOpen(false); setViewingWf(null); }}
          destroyOnHidden
        >
          {viewingWf && (
            <div>
              <p><strong>ID:</strong> {viewingWf.id}</p>
              <p><strong>identifier:</strong> {viewingWf.identifier}</p>
              <p><strong>name:</strong> {viewingWf.name}</p>
              <p><strong>description:</strong> {viewingWf.description || '—'}</p>
              <p><strong>timeout_ms:</strong> {viewingWf.timeout_ms}</p>
              <p>
                <strong>required_capabilities:</strong>{' '}
                {viewingWf.required_capabilities?.map((c) => (
                  <Tag key={c}>{c}</Tag>
                )) ?? '—'}
              </p>
              <p>
                <strong>tags:</strong>{' '}
                {viewingWf.tags?.map((t) => (
                  <Tag key={t.id}>{t.name}</Tag>
                )) ?? '—'}
              </p>
              <p><strong>created_at:</strong> {formatDate(viewingWf.created_at)}</p>
              <p><strong>updated_at:</strong> {formatDate(viewingWf.updated_at)}</p>
            </div>
          )}
        </Drawer>

        {/* Create Modal */}
        <Modal
          title="新建 Workflow"
          open={createOpen}
          onCancel={() => setCreateOpen(false)}
          onOk={() => createForm.submit()}
          destroyOnHidden
          width={560}
        >
          <Form
            form={createForm}
            layout="vertical"
            initialValues={{ timeout_ms: 33000 }}
            onFinish={onCreate}
          >
            <Form.Item name="identifier" label="identifier" rules={[{ required: true }]}>
              <Input placeholder="ingest" />
            </Form.Item>
            <Form.Item name="name" label="name" rules={[{ required: true }]}>
              <Input placeholder="Ingest pipeline" />
            </Form.Item>
            <Form.Item name="description" label="description">
              <Input.TextArea rows={2} />
            </Form.Item>
            <Form.Item name="timeout_ms" label="timeout_ms (默认 33000)">
              <InputNumber min={1000} max={330000} style={{ width: '100%' }} />
            </Form.Item>
            <Form.Item name="category_id" label="分类">
              <CategoryTreeSelect placeholder="选择分类" />
            </Form.Item>
            <Form.Item name="required_capabilities" label="权限要求" tooltip="该 workflow 执行时需要的能力，执行时会校验 Agent 是否被授权">
              <Select
                mode="multiple"
                placeholder="选择所需权限"
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
              />
            </Form.Item>
            <Form.Item name="tag_ids" label="标签">
              <Select
                mode="multiple"
                placeholder="选择标签"
                allowClear
                options={allTags.map((t) => ({ value: t.id, label: t.name }))}
              />
            </Form.Item>
          </Form>
        </Modal>

        {/* Edit Modal */}
        <Modal
          title="编辑 Workflow 基本信息"
          open={editOpen}
          onCancel={() => {
            setEditOpen(false);
            setEditingWf(null);
          }}
          onOk={() => editForm.submit()}
          destroyOnHidden
          width={560}
        >
          <Form form={editForm} layout="vertical" onFinish={onEdit}>
            <Form.Item name="name" label="name" rules={[{ required: true }]}>
              <Input />
            </Form.Item>
            <Form.Item name="description" label="description">
              <Input.TextArea rows={3} />
            </Form.Item>
            <Form.Item name="timeout_ms" label="timeout_ms">
              <InputNumber min={1000} max={330000} style={{ width: '100%' }} />
            </Form.Item>
            <Form.Item name="category_id" label="分类">
              <CategoryTreeSelect placeholder="选择分类" />
            </Form.Item>
            <Form.Item name="required_capabilities" label="权限要求" tooltip="该 workflow 执行时需要的能力，执行时会校验 Agent 是否被授权">
              <Select
                mode="multiple"
                placeholder="选择所需权限"
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
              />
            </Form.Item>
            <Form.Item name="tag_ids" label="标签">
              <Select
                mode="multiple"
                placeholder="选择标签"
                allowClear
                options={allTags.map((t) => ({ value: t.id, label: t.name }))}
              />
            </Form.Item>
          </Form>
        </Modal>
      </div>
    </div>
  );
}
