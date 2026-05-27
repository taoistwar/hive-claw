// FunctionPage — 列表 + 左侧分类树过滤 + 创建 + 编辑 + 删除 (T092 扩展)

import { useEffect, useState } from 'react';
import { Table, Tag, Space, Input, Select, message, Button, Popconfirm, Tree, Drawer } from 'antd';
import type { DataNode } from 'antd/es/tree';
import { PlusOutlined, EditOutlined, DeleteOutlined, PlayCircleOutlined, EyeOutlined } from '@ant-design/icons';
import type { ColumnsType } from 'antd/es/table';
import { listFunctions, deleteFunction, type FunctionItem, type FunctionList } from '../services/function';
import FunctionForm from '../components/FunctionForm';
import FunctionTester from '../components/FunctionTester';
import { FunctionDetail } from '../components/FunctionDetail';
import { listCategoriesTree, type CategoryNode } from '../services/category';

type FormMode = 'create' | 'edit' | null;

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
  const [search, setSearch] = useState('');
  const [kind, setKind] = useState<'builtin' | 'custom' | ''>('');
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

  const fetchData = () => {
    setLoading(true);
    listFunctions({
      search: search || undefined,
      kind: kind || undefined,
      category_id: categoryId,
    })
      .then(setData)
      .catch((e) => message.error(`加载失败：${(e as Error).message}`))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    fetchData();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [search, kind, categoryId]);

  const handleCategorySelect = (key?: React.Key) => {
    const newSelected = key === selectedCategoryKey ? undefined : key;
    setSelectedCategoryKey(newSelected);
    setCategoryId(newSelected !== undefined ? Number(newSelected) : undefined);
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
          {record.kind !== 1 && (
            <>
              <Button
                type="link"
                size="small"
                icon={<EditOutlined />}
                onClick={() => handleEdit(record)}
              >
                编辑
              </Button>
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
            </>
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
          <Input.Search
            placeholder="搜索 function"
            allowClear
            onSearch={setSearch}
            style={{ width: 280 }}
            aria-label="搜索 function"
          />
          <Select
            value={kind}
            onChange={(v) => setKind(v)}
            style={{ width: 140 }}
            aria-label="按 kind 筛选"
            options={[
              { value: '', label: '全部 kind' },
              { value: 'builtin', label: 'builtin' },
              { value: 'custom', label: 'custom' },
            ]}
          />
          <Button
            type="primary"
            icon={<PlusOutlined />}
            onClick={handleCreate}
          >
            创建函数
          </Button>
        </Space>
        <Table<FunctionItem>
          rowKey="id"
          columns={columns}
          dataSource={data.items}
          loading={loading}
          pagination={{ total: data.total, pageSize: data.limit }}
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
