// TagPage — Tag 列表 + 创建 + 编辑 + 删除（引用阻塞 4091）(T136)

import { useCallback, useEffect, useState } from 'react';
import { Button, Drawer, Form, Input, Modal, Space, Table, Tag as AntTag, message } from 'antd';
import type { ColumnsType } from 'antd/es/table';
import { ColorPickerInput } from '../components/ColorPickerInput';
import { createTag, deleteTag, listTags, updateTag, type TagItem } from '../services/tag';

const DEFAULT_PAGE_SIZE = 5;

export default function TagPage() {
  const [items, setItems] = useState<TagItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [editOpen, setEditOpen] = useState(false);
  const [editingTag, setEditingTag] = useState<TagItem | null>(null);
  const [search, setSearch] = useState('');
  const [pagination, setPagination] = useState({
    current: 1,
    pageSize: DEFAULT_PAGE_SIZE,
    total: 0,
  });
  const [createForm] = Form.useForm();
  const [editForm] = Form.useForm();

  const doFetch = useCallback(async (page: number, currentSearch: string, currentPageSize: number) => {
    const offset = (page - 1) * currentPageSize;
    setLoading(true);
    try {
      const result = await listTags(currentSearch || undefined, offset, currentPageSize);
      setItems(result.items);
      setPagination((prev) => ({ ...prev, current: page, total: result.total }));
    } catch (e) {
      void message.error(`加载失败：${(e as Error).message}`);
    } finally {
      setLoading(false);
    }
  }, []);

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

  const handleSearch = useCallback((value: string) => {
    setSearch(value);
    setPagination((prev) => ({ ...prev, current: 1 }));
    doFetch(1, value, pagination.pageSize);
  }, [pagination.pageSize, doFetch]);

  const onCreate = async (values: { name: string; color?: string }) => {
    try {
      await createTag(values);
      void message.success('已创建');
      createForm.resetFields();
      setCreateOpen(false);
      await refresh();
    } catch (e) {
      void message.error(`创建失败：${(e as Error).message}`);
    }
  };

  const openEdit = (tag: TagItem) => {
    setEditingTag(tag);
    editForm.setFieldsValue({ name: tag.name, color: tag.color });
    setEditOpen(true);
  };

  const onEdit = async (values: { name: string; color?: string }) => {
    if (!editingTag) return;
    try {
      await updateTag(editingTag.id, values);
      void message.success('更新成功');
      editForm.resetFields();
      setEditOpen(false);
      setEditingTag(null);
      await refresh();
    } catch (e) {
      void message.error(`更新失败：${(e as Error).message}`);
    }
  };

  const onDelete = (t: TagItem) => {
    Modal.confirm({
      title: `删除标签「${t.name}」？`,
      content:
        t.reference_count > 0
          ? `当前被 ${t.reference_count} 个对象引用，删除会被拒绝（4091）`
          : '当前未被引用，可以安全删除',
      okButtonProps: { danger: true },
      onOk: async () => {
        try {
          await deleteTag(t.id);
          void message.success('已删除');
          await refresh();
        } catch (e: unknown) {
          const err = e as { response?: { data?: { code?: number; message?: string } } };
          if (err.response?.data?.code === 4091) {
            void message.error(err.response.data.message ?? '被引用，无法删除');
          } else {
            void message.error(`删除失败：${(e as Error).message}`);
          }
        }
      },
    });
  };

  const columns: ColumnsType<TagItem> = [
    { title: 'ID', dataIndex: 'id', width: 60 },
    {
      title: 'name',
      dataIndex: 'name',
      render: (n: string, r) => <AntTag color={r.color ?? 'default'}>{n}</AntTag>,
    },
    {
      title: 'color',
      dataIndex: 'color',
      render: (c: string | null) => (
        <Space>
          {c && (
            <span
              style={{
                display: 'inline-block',
                width: 16,
                height: 16,
                borderRadius: 4,
                backgroundColor: c,
                border: '1px solid #d9d9d9',
              }}
            />
          )}
          <span>{c ?? '—'}</span>
        </Space>
      ),
    },
    { title: '被引用', dataIndex: 'reference_count', width: 100 },
    {
      title: 'actions',
      key: 'actions',
      render: (_, r) => (
        <Space>
          <Button size="small" onClick={() => openEdit(r)}>
            编辑
          </Button>
          <Button danger size="small" onClick={() => onDelete(r)}>
            删除
          </Button>
        </Space>
      ),
    },
  ];

  return (
    <div>
      <Space style={{ marginBottom: 16 }}>
        <Button type="primary" onClick={() => setCreateOpen(true)}>
          新建 Tag
        </Button>
        <Input.Search
          placeholder="搜索 tag name"
          allowClear
          onSearch={handleSearch}
          style={{ width: 280 }}
          aria-label="搜索 tag"
        />
      </Space>
      <Table<TagItem>
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

      <Modal
        title="新建 Tag"
        open={createOpen}
        onCancel={() => setCreateOpen(false)}
        onOk={() => createForm.submit()}
        destroyOnHidden
      >
        <Form form={createForm} layout="vertical" onFinish={onCreate}>
          <Form.Item name="name" label="name" rules={[{ required: true }]}>
            <Input />
          </Form.Item>
          <Form.Item name="color" label="color (可选)">
            <ColorPickerInput />
          </Form.Item>
        </Form>
      </Modal>

      <Drawer
        title={editingTag ? `编辑 Tag「${editingTag.name}」` : '编辑 Tag'}
        open={editOpen}
        width={400}
        onClose={() => {
          setEditOpen(false);
          setEditingTag(null);
        }}
      >
        {editingTag && (
          <Form form={editForm} layout="vertical" onFinish={onEdit}>
            <Form.Item name="name" label="name" rules={[{ required: true }]}>
              <Input />
            </Form.Item>
            <Form.Item name="color" label="color (可选)">
              <ColorPickerInput />
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
