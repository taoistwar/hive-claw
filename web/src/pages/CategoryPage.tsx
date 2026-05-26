// CategoryPage — 树形展示 + CRUD (T136)

import { useCallback, useEffect, useState } from 'react';
import { Button, Form, Input, InputNumber, Modal, Space, Tree, message } from 'antd';
import type { DataNode } from 'antd/es/tree';
import {
  createCategory,
  deleteCategory,
  listCategoriesTree,
  type CategoryNode,
} from '../services/category';

function toDataNodes(nodes: CategoryNode[]): DataNode[] {
  return nodes.map((n) => ({
    key: n.id,
    title: `${n.name} (${n.slug})`,
    children: n.children?.length ? toDataNodes(n.children) : undefined,
  }));
}

export default function CategoryPage() {
  const [data, setData] = useState<CategoryNode[]>([]);
  const [loading, setLoading] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [form] = Form.useForm();

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      setData(await listCategoriesTree());
    } catch (e) {
      void message.error(`加载失败：${(e as Error).message}`);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const onCreate = async (values: {
    name: string;
    slug: string;
    parent_id?: number;
    description?: string;
  }) => {
    try {
      await createCategory(values);
      void message.success('创建成功');
      form.resetFields();
      setCreateOpen(false);
      await refresh();
    } catch (e) {
      void message.error(`创建失败：${(e as Error).message}`);
    }
  };

  const onDelete = (key: React.Key) => {
    Modal.confirm({
      title: `删除 category id=${key}？`,
      content: '子分类会失去父分类绑定（不会一并删除）',
      onOk: async () => {
        try {
          await deleteCategory(key as number);
          void message.success('已删除');
          await refresh();
        } catch (e) {
          void message.error(`删除失败：${(e as Error).message}`);
        }
      },
    });
  };

  return (
    <div>
      <Space style={{ marginBottom: 16 }}>
        <Button type="primary" onClick={() => setCreateOpen(true)}>
          新建 Category
        </Button>
      </Space>
      <Tree
        treeData={toDataNodes(data)}
        loadData={async () => {}}
        defaultExpandAll
        showLine
        titleRender={(n) => (
          <Space>
            <span>{String(n.title)}</span>
            <Button size="small" danger type="link" onClick={() => onDelete(n.key)}>
              删除
            </Button>
          </Space>
        )}
      />
      {loading && <p>加载中…</p>}

      <Modal
        title="新建 Category"
        open={createOpen}
        onCancel={() => setCreateOpen(false)}
        onOk={() => form.submit()}
        destroyOnClose
      >
        <Form form={form} layout="vertical" onFinish={onCreate}>
          <Form.Item name="name" label="name" rules={[{ required: true }]}>
            <Input />
          </Form.Item>
          <Form.Item name="slug" label="slug" rules={[{ required: true }]}>
            <Input />
          </Form.Item>
          <Form.Item name="parent_id" label="parent_id (留空 = 顶级)">
            <InputNumber min={1} />
          </Form.Item>
          <Form.Item name="description" label="description">
            <Input.TextArea rows={2} />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
