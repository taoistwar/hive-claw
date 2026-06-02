import { useCallback, useEffect, useState } from 'react';
import {
  Button,
  Drawer,
  Form,
  Input,
  Modal,
  Select,
  Space,
  Switch,
  Table,
  Tooltip,
  Typography,
  message,
} from 'antd';
import type { ColumnsType } from 'antd/es/table';
import {
  createGlobalConfig,
  deleteGlobalConfig,
  listGlobalConfigs,
  updateGlobalConfig,
  type GlobalConfigItem,
} from '../services/globalConfig';
import { useAuth } from '../hooks/useAuth';

const { Text } = Typography;

const DEFAULT_PAGE_SIZE = 10;

const CONFIG_TYPES = [
  { label: '数字 (number)', value: 'number' },
  { label: '字符串 (string)', value: 'string' },
  { label: '布尔 (boolean)', value: 'boolean' },
  { label: 'JSON (json)', value: 'json' },
];

function renderDataValue(item: GlobalConfigItem) {
  const v = item.data?.value;
  if (v === undefined || v === null) return <Text type="secondary">—</Text>;

  switch (item.config_type) {
    case 'boolean':
      return (
        <Switch
          checked={!!v}
          disabled
          size="small"
          style={{ pointerEvents: 'none' }}
        />
      );
    case 'number':
      return <Text code>{String(v)}</Text>;
    case 'json':
      return (
        <Text
          code
          ellipsis={{ tooltip: JSON.stringify(v, null, 2) }}
          style={{ maxWidth: 320, display: 'inline-block' }}
        >
          {JSON.stringify(v)}
        </Text>
      );
    default:
      return <Text>{String(v)}</Text>;
  }
}

export default function GlobalConfigPage() {
  const { admin } = useAuth();
  const isSuper = admin?.role === 3;
  const isSystemOrSuper = admin?.role === 2 || admin?.role === 3;
  // Normal (role=1): 只能查看，无操作按钮

  const [items, setItems] = useState<GlobalConfigItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [editOpen, setEditOpen] = useState(false);
  const [editingItem, setEditingItem] = useState<GlobalConfigItem | null>(null);
  const [search, setSearch] = useState('');
  const [pagination, setPagination] = useState({
    current: 1,
    pageSize: DEFAULT_PAGE_SIZE,
    total: 0,
  });
  const [createForm] = Form.useForm();
  const [editForm] = Form.useForm();

  const doFetch = useCallback(
    async (page: number, currentSearch: string, currentPageSize: number) => {
      const offset = (page - 1) * currentPageSize;
      setLoading(true);
      try {
        const result = await listGlobalConfigs(
          currentSearch || undefined,
          offset,
          currentPageSize,
        );
        setItems(result.items);
        setPagination((prev) => ({ ...prev, current: page, total: result.total }));
      } catch (e) {
        void message.error(`加载失败：${(e as Error).message}`);
      } finally {
        setLoading(false);
      }
    },
    [],
  );

  const refresh = useCallback(async () => {
    doFetch(pagination.current, search, pagination.pageSize);
  }, [pagination.current, pagination.pageSize, search, doFetch]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const handlePaginationChange = (page: number, pageSize: number) => {
    setPagination((prev) => ({ ...prev, current: page, pageSize }));
    doFetch(page, search, pageSize);
  };

  const handleSearch = useCallback(
    (value: string) => {
      setSearch(value);
      setPagination((prev) => ({ ...prev, current: 1 }));
      doFetch(1, value, pagination.pageSize);
    },
    [pagination.pageSize, doFetch],
  );

  const onCreate = async (values: {
    name: string;
    key: string;
    config_type: string;
    raw_value: string;
  }) => {
    try {
      let data: Record<string, unknown>;
      switch (values.config_type) {
        case 'number':
          data = { value: Number(values.raw_value) };
          break;
        case 'boolean':
          data = { value: values.raw_value === 'true' };
          break;
        case 'json':
          data = JSON.parse(values.raw_value);
          break;
        default:
          data = { value: values.raw_value };
      }

      await createGlobalConfig({
        name: values.name,
        key: values.key,
        config_type: values.config_type,
        data,
      });
      void message.success('已创建');
      createForm.resetFields();
      setCreateOpen(false);
      await refresh();
    } catch (e) {
      void message.error(`创建失败：${(e as Error).message}`);
    }
  };

  const openEdit = (item: GlobalConfigItem) => {
    setEditingItem(item);

    const initialType = item.config_type;
    let rawValue: unknown = item.data?.value;

    if (initialType === 'json') {
      rawValue = JSON.stringify(item.data, null, 2);
    }

    editForm.setFieldsValue({
      name: item.name,
      config_type: item.config_type,
      raw_value: rawValue !== undefined ? String(rawValue) : '',
    });
    setEditOpen(true);
  };

  const onEdit = async (values: { name: string; config_type: string; raw_value: string }) => {
    if (!editingItem) return;
    try {
      let data: Record<string, unknown>;
      switch (values.config_type) {
        case 'number':
          data = { value: Number(values.raw_value) };
          break;
        case 'boolean':
          data = { value: values.raw_value === 'true' };
          break;
        case 'json':
          data = JSON.parse(values.raw_value);
          break;
        default:
          data = { value: values.raw_value };
      }

      await updateGlobalConfig(editingItem.id, {
        name: values.name,
        config_type: values.config_type,
        data,
      });
      void message.success('更新成功');
      editForm.resetFields();
      setEditOpen(false);
      setEditingItem(null);
      await refresh();
    } catch (e) {
      void message.error(`更新失败：${(e as Error).message}`);
    }
  };

  const onDelete = (item: GlobalConfigItem) => {
    if (!isSuper) {
      void message.error('仅超级管理员可删除全局配置');
      return;
    }

    Modal.confirm({
      title: `删除配置「${item.name}」？`,
      content: `Key: ${item.key}，此操作不可撤销。`,
      okButtonProps: { danger: true },
      onOk: async () => {
        try {
          await deleteGlobalConfig(item.id);
          void message.success('已删除');
          await refresh();
        } catch (e) {
          void message.error(`删除失败：${(e as Error).message}`);
        }
      },
    });
  };

  const columns: ColumnsType<GlobalConfigItem> = [
    { title: 'ID', dataIndex: 'id', width: 60 },
    { title: '名称', dataIndex: 'name', width: 160 },
    {
      title: 'Key',
      dataIndex: 'key',
      width: 180,
      render: (k: string) => <Text code>{k}</Text>,
    },
    {
      title: '类型',
      dataIndex: 'config_type',
      width: 100,
      render: (t: string) => {
        const found = CONFIG_TYPES.find((c) => c.value === t);
        return <Text>{found?.label ?? t}</Text>;
      },
    },
    {
      title: '值',
      dataIndex: 'data',
      render: (_, r) => renderDataValue(r),
    },
    {
      title: '更新时间',
      dataIndex: 'updated_at',
      width: 180,
      render: (t: string) => new Date(t).toLocaleString('zh-CN'),
    },
    {
      title: '操作',
      key: 'actions',
      width: 160,
      render: (_, r) => (
        <Space>
          {isSystemOrSuper ? (
            <Button size="small" onClick={() => openEdit(r)}>
              编辑
            </Button>
          ) : (
            <Tooltip title="仅系统管理员及以上可编辑">
              <Button size="small" disabled>
                编辑
              </Button>
            </Tooltip>
          )}
          {isSuper ? (
            <Button danger size="small" onClick={() => onDelete(r)}>
              删除
            </Button>
          ) : (
            <Tooltip title="仅超级管理员可删除">
              <Button danger size="small" disabled>
                删除
              </Button>
            </Tooltip>
          )}
        </Space>
      ),
    },
  ];

  return (
    <div>
      <Space style={{ marginBottom: 16 }}>
        {isSuper ? (
          <Button type="primary" onClick={() => setCreateOpen(true)}>
            新建配置
          </Button>
        ) : (
          <Tooltip title="仅超级管理员可创建">
            <Button type="primary" disabled>
              新建配置
            </Button>
          </Tooltip>
        )}
        <Input.Search
          placeholder="搜索名称或 Key"
          allowClear
          onSearch={handleSearch}
          style={{ width: 300 }}
          aria-label="搜索全局配置"
        />
      </Space>

      <Table<GlobalConfigItem>
        rowKey="id"
        columns={columns}
        dataSource={items}
        loading={loading}
        pagination={{
          current: pagination.current,
          pageSize: pagination.pageSize,
          total: pagination.total,
          showSizeChanger: true,
          showTotal: (total) => `共 ${total} 条`,
          onChange: handlePaginationChange,
        }}
      />

      {/* Create Modal */}
      <Modal
        title="新建全局配置"
        open={createOpen}
        onCancel={() => setCreateOpen(false)}
        onOk={() => createForm.submit()}
        destroyOnHidden
      >
        <Form form={createForm} layout="vertical" onFinish={onCreate}>
          <Form.Item name="name" label="名称" rules={[{ required: true, message: '请输入名称' }]}>
            <Input placeholder="例如：非会员次数" />
          </Form.Item>
          <Form.Item name="key" label="Key" rules={[{ required: true, message: '请输入 Key' }]}>
            <Input placeholder="例如：normal_ask_times" />
          </Form.Item>
          <Form.Item
            name="config_type"
            label="类型"
            rules={[{ required: true, message: '请选择类型' }]}
            initialValue="string"
          >
            <Select options={CONFIG_TYPES} />
          </Form.Item>
          <Form.Item
            name="raw_value"
            label="值"
            rules={[{ required: true, message: '请输入值' }]}
          >
            <Input placeholder="字符串直接输入；JSON 请输入合法 JSON 字符串" />
          </Form.Item>
        </Form>
      </Modal>

      {/* Edit Drawer */}
      <Drawer
        title={editingItem ? `编辑配置「${editingItem.name}」` : '编辑配置'}
        open={editOpen}
        width={420}
        onClose={() => {
          setEditOpen(false);
          setEditingItem(null);
        }}
      >
        {editingItem && (
          <Form form={editForm} layout="vertical" onFinish={onEdit}>
            <Form.Item name="name" label="名称" rules={[{ required: true }]}>
              <Input disabled={!isSuper} />
            </Form.Item>
            <Form.Item
              name="config_type"
              label="类型"
              rules={[{ required: true }]}
            >
              <Select options={CONFIG_TYPES} disabled={!isSuper} />
            </Form.Item>
            <Form.Item
              name="raw_value"
              label="值"
              rules={[{ required: true }]}
            >
              <Input.TextArea
                rows={4}
                placeholder="字符串直接输入；JSON 请输入合法 JSON 字符串"
              />
            </Form.Item>
            <Form.Item>
              <Button type="primary" htmlType="submit">
                保存
              </Button>
            </Form.Item>
          </Form>
        )}
      </Drawer>
    </div>
  );
}
