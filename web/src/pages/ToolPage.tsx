import { useEffect, useState } from 'react';
import { Typography, Button, Table, Tag, Form, Input, Select, Space, DatePicker, Popconfirm, Tree, message } from 'antd';
import type { DataNode } from 'antd/es/tree';
import { PlusOutlined, EditOutlined, DeleteOutlined, SearchOutlined, ReloadOutlined, EyeOutlined, ExperimentOutlined } from '@ant-design/icons';
import type { ColumnsType } from 'antd/es/table';
import { useTool } from '../hooks/useTool';
import ToolForm from '../components/ToolForm';
import ToolDetail from '../components/ToolDetail';
import ToolTestModal from '../components/ToolTestModal';
import type { ToolItem, ToolSearchParams } from '../services/tool';
import { listCategoriesTree, type CategoryNode } from '../services/category';
import { listFunctions, type FunctionItem } from '../services/function';
import { listWorkflows } from '../services/workflow';

const { Title } = Typography;
const { RangePicker } = DatePicker;

const KIND_MAP: Record<number, { label: string; color: string }> = {
  1: { label: 'function-wrap', color: 'green' },
  2: { label: 'workflow-wrap', color: 'cyan' },
};

const SOURCE_MAP: Record<string, { label: string; color: string }> = {
  builtin: { label: 'builtin', color: 'purple' },
  workspace: { label: 'workspace', color: 'blue' },
};

const ALWAYS_TAG = { label: 'always', color: 'red' };

function toDataNodes(nodes: CategoryNode[]): DataNode[] {
  return nodes.map((n) => ({
    key: n.id,
    title: n.name,
    children: n.children?.length ? toDataNodes(n.children) : undefined,
  }));
}

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
  const [testModalVisible, setTestModalVisible] = useState(false);
  const [testingTool, setTestingTool] = useState<ToolItem | null>(null);
  const [categoryTree, setCategoryTree] = useState<CategoryNode[]>([]);
  const [selectedCategoryKey, setSelectedCategoryKey] = useState<React.Key | undefined>(undefined);
  const [categoryId, setCategoryId] = useState<number | undefined>(undefined);
  const [treeExpandedKeys, setTreeExpandedKeys] = useState<React.Key[]>([]);
  const [treeLoading, setTreeLoading] = useState(false);
  const [functionMap, setFunctionMap] = useState<Map<number, string>>(new Map());
  const [workflowMap, setWorkflowMap] = useState<Map<number, string>>(new Map());

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

    listFunctions({ limit: 500 })
      .then((res) => {
        const map = new Map<number, string>();
        res.items.forEach((f: FunctionItem) => map.set(f.id, f.name));
        setFunctionMap(map);
      })
      .catch(() => {});

    listWorkflows()
      .then((res) => {
        const map = new Map<number, string>();
        res.items.forEach((w) => map.set(w.id, w.name));
        setWorkflowMap(map);
      })
      .catch(() => {});

    fetchTools();
  }, []);

  const handleCategorySelect = (key?: React.Key) => {
    const newSelected = key === selectedCategoryKey ? undefined : key;
    setSelectedCategoryKey(newSelected);
    const newCategoryId = newSelected !== undefined ? Number(newSelected) : undefined;
    setCategoryId(newCategoryId);
    const params: ToolSearchParams = {
      category_id: newCategoryId,
      search: form.getFieldValue('search') || undefined,
      kind: form.getFieldValue('kind'),
      source: form.getFieldValue('source') || undefined,
    };
    setSearchParams(params);
    handleSearch(params);
  };

  const handlePaginationChange = (page: number, pageSize: number) => {
    setPagination((prev) => ({ ...prev, current: page, pageSize }));
  };

  const doSearch = () => {
    const values = form.getFieldsValue();
    const params: ToolSearchParams = {
      search: values.search || undefined,
      kind: values.kind,
      source: values.source || undefined,
      category_id: categoryId,
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
      title: '来源',
      dataIndex: 'source',
      width: 110,
      render: (s: string) => {
        const info = SOURCE_MAP[s];
        return info ? <Tag color={info.color}>{info.label}</Tag> : s;
      },
    },
    {
      title: 'always',
      dataIndex: 'is_always',
      width: 80,
      render: (v: boolean) => (v ? <Tag color={ALWAYS_TAG.color}>{ALWAYS_TAG.label}</Tag> : '-'),
    },
    {
      title: 'required_capabilities',
      dataIndex: 'required_capabilities',
      width: 180,
      render: (caps: string[] | null) =>
        caps?.map((c) => <Tag key={c}>{c}</Tag>) ?? '-',
    },
    {
      title: '标签',
      dataIndex: 'tags',
      width: 180,
      render: (tags: { id: number; name: string }[]) =>
        tags && tags.length > 0
          ? tags.map((t) => <Tag key={t.id}>{t.name}</Tag>)
          : '-',
    },
    {
      title: 'function_id',
      dataIndex: 'function_id',
      width: 140,
      render: (v: number | null) => {
        if (!v) return '-';
        const name = functionMap.get(v);
        return name ? `${name}[${v}]` : v;
      },
    },
    {
      title: 'workflow_id',
      dataIndex: 'workflow_id',
      width: 140,
      render: (v: number | null) => {
        if (!v) return '-';
        const name = workflowMap.get(v);
        return name ? `${name}[${v}]` : v;
      },
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
      width: 260,
      render: (_: unknown, record: ToolItem) => (
        <span>
          <Button
            type="link"
            size="small"
            icon={<ExperimentOutlined />}
            onClick={() => { setTestingTool(record); setTestModalVisible(true); }}
          >
            测试
          </Button>
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
            description={record.source === 'builtin' ? '内置工具不可删除' : '确定要删除该工具吗？'}
            onConfirm={() => record.source !== 'builtin' && handleDelete(record.id)}
            okText="确定"
            cancelText="取消"
          >
            <Button type="link" size="small" danger icon={<DeleteOutlined />} disabled={record.source === 'builtin'}>
              删除
            </Button>
          </Popconfirm>
        </span>
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

      <div
        style={{
          display: 'flex',
          flexWrap: 'wrap',
          gap: 12,
          alignItems: 'flex-end',
          marginBottom: 16,
        }}
      >
        <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
          <label style={{ fontSize: 12, color: '#666' }}>搜索</label>
          <Input
            value={form.getFieldValue('search')}
            onChange={(e) => form.setFieldValue('search', e.target.value)}
            placeholder="搜索名称/标识符/描述"
            allowClear
            style={{ width: 200 }}
          />
        </div>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
          <label style={{ fontSize: 12, color: '#666' }}>类型</label>
          <Select
            value={form.getFieldValue('kind')}
            onChange={(v) => form.setFieldValue('kind', v)}
            placeholder="类型"
            allowClear
            style={{ width: 140 }}
          >
            <Select.Option value={1}>function-wrap</Select.Option>
            <Select.Option value={2}>workflow-wrap</Select.Option>
          </Select>
        </div>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
          <label style={{ fontSize: 12, color: '#666' }}>来源</label>
          <Select
            value={form.getFieldValue('source')}
            onChange={(v) => form.setFieldValue('source', v)}
            placeholder="来源"
            allowClear
            style={{ width: 130 }}
          >
            <Select.Option value="workspace">workspace</Select.Option>
            <Select.Option value="builtin">builtin</Select.Option>
          </Select>
        </div>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
          <label style={{ fontSize: 12, color: '#666' }}>创建时间</label>
          <RangePicker
            value={form.getFieldValue('created_at_range')}
            onChange={(v) => form.setFieldValue('created_at_range', v)}
            placeholder={['创建时间起', '创建时间止']}
            style={{ width: 260 }}
          />
        </div>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
          <label style={{ fontSize: 12, color: '#666' }}>修改时间</label>
          <RangePicker
            value={form.getFieldValue('updated_at_range')}
            onChange={(v) => form.setFieldValue('updated_at_range', v)}
            placeholder={['修改时间起', '修改时间止']}
            style={{ width: 260 }}
          />
        </div>
        <div style={{ display: 'flex', gap: 8 }}>
          <Button type="primary" icon={<SearchOutlined />} onClick={() => form.submit()}>
            搜索
          </Button>
          <Button icon={<ReloadOutlined />} onClick={handleResetSearch}>
            重置
          </Button>
        </div>
      </div>

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

      <ToolTestModal
        visible={testModalVisible}
        toolId={testingTool?.id ?? null}
        toolName={testingTool?.name ?? ''}
        onCancel={() => { setTestModalVisible(false); setTestingTool(null); }}
      />
      </div>
    </div>
  );
};

export default ToolPage;
