// CategoryPage — 树形展示 + CRUD (T136)

import { useCallback, useEffect, useState } from 'react';
import { Button, Form, Input, Modal, Space, Tree, TreeSelect, message } from 'antd';
import type { DataNode } from 'antd/es/tree';
import {
  createCategory,
  deleteCategory,
  listCategoriesTree,
  listCategoriesFlat,
  updateCategory,
  type CategoryNode,
  type CategoryItem,
} from '../services/category';

function toDataNodes(nodes: CategoryNode[]): DataNode[] {
  return nodes.map((n) => ({
    key: n.id,
    title: `${n.name} (${n.slug})`,
    children: n.children?.length ? toDataNodes(n.children) : undefined,
  }));
}

function toTreeSelectOptions(nodes: CategoryNode[]): any[] {
  return nodes.map((n) => ({
    value: n.id,
    title: n.name,
    children: n.children?.length ? toTreeSelectOptions(n.children) : undefined,
  }));
}

function getAllKeys(nodes: DataNode[]): React.Key[] {
  const keys: React.Key[] = [];
  const collect = (nds: DataNode[]) => {
    nds.forEach((n) => {
      keys.push(n.key);
      if (n.children && n.children.length > 0) {
        collect(n.children);
      }
    });
  };
  collect(nodes);
  return keys;
}

export default function CategoryPage() {
  const [data, setData] = useState<CategoryNode[]>([]);
  const [flatData, setFlatData] = useState<CategoryItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [expandedKeys, setExpandedKeys] = useState<React.Key[]>([]);
  const [createOpen, setCreateOpen] = useState(false);
  const [editOpen, setEditOpen] = useState(false);
  const [editingCategory, setEditingCategory] = useState<CategoryItem | null>(null);
  const [createForm] = Form.useForm();
  const [editForm] = Form.useForm();

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const treeData = await listCategoriesTree();
      setData(treeData);
      setFlatData(await listCategoriesFlat());
      setExpandedKeys(getAllKeys(toDataNodes(treeData)));
    } catch (e) {
      void message.error(`加载失败：${(e as Error).message}`);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const expandAll = () => {
    setExpandedKeys(getAllKeys(toDataNodes(data)));
  };

  const collapseAll = () => {
    setExpandedKeys([]);
  };

  const onCreate = async (values: {
    name: string;
    slug: string;
    parent_id?: number;
    description?: string;
  }) => {
    try {
      await createCategory(values);
      void message.success('创建成功');
      createForm.resetFields();
      setCreateOpen(false);
      await refresh();
    } catch (e) {
      void message.error(`创建失败：${(e as Error).message}`);
    }
  };

  const onEdit = async (values: {
    name: string;
    slug: string;
    parent_id?: number | null;
    description?: string;
  }) => {
    if (!editingCategory) return;
    try {
      await updateCategory(editingCategory.id, {
        ...values,
        updated_at: editingCategory.updated_at,
      });
      void message.success('更新成功');
      editForm.resetFields();
      setEditOpen(false);
      setEditingCategory(null);
      await refresh();
    } catch (e) {
      void message.error(`更新失败：${(e as Error).message}`);
    }
  };

  const openEdit = (category: CategoryItem) => {
    setEditingCategory(category);
    editForm.setFieldsValue({
      name: category.name,
      slug: category.slug,
      parent_id: category.parent_id,
      description: category.description,
    });
    setEditOpen(true);
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

  const treeSelectOptions = [
    { value: undefined, title: '无 (顶级分类)' },
    ...toTreeSelectOptions(data),
  ];

  return (
    <div>
      <Space style={{ marginBottom: 16 }}>
        <Button type="primary" onClick={() => setCreateOpen(true)}>
          新建 Category
        </Button>
        <Button icon="📂" onClick={expandAll}>
          全部展开
        </Button>
        <Button icon="" onClick={collapseAll}>
          全部折叠
        </Button>
      </Space>
      <Tree
        treeData={toDataNodes(data)}
        expandedKeys={expandedKeys}
        onExpand={(keys) => setExpandedKeys(keys)}
        loadData={async () => {}}
        showLine
        titleRender={(n) => (
          <Space>
            <span>{String(n.title)}</span>
            <Button size="small" type="link" onClick={() => {
              const item = flatData.find(c => c.id === n.key);
              if (item) openEdit(item);
            }}>
              编辑
            </Button>
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
        onOk={() => createForm.submit()}
        destroyOnHidden
      >
        <Form form={createForm} layout="vertical" onFinish={onCreate}>
          <Form.Item name="name" label="name" rules={[{ required: true }]}>
            <Input />
          </Form.Item>
          <Form.Item name="slug" label="slug" rules={[{ required: true }]}>
            <Input />
          </Form.Item>
          <Form.Item name="parent_id" label="parent_id">
            <TreeSelect
              treeData={treeSelectOptions}
              placeholder="选择父级分类或留空作为顶级分类"
              allowClear
              treeDefaultExpandAll
            />
          </Form.Item>
          <Form.Item name="description" label="description">
            <Input.TextArea rows={2} />
          </Form.Item>
        </Form>
      </Modal>

      <Modal
        title="编辑 Category"
        open={editOpen}
        onCancel={() => {
          setEditOpen(false);
          setEditingCategory(null);
        }}
        onOk={() => editForm.submit()}
        destroyOnHidden
      >
        <Form form={editForm} layout="vertical" onFinish={onEdit}>
          <Form.Item name="name" label="name" rules={[{ required: true }]}>
            <Input />
          </Form.Item>
          <Form.Item name="slug" label="slug" rules={[{ required: true }]}>
            <Input />
          </Form.Item>
          <Form.Item name="parent_id" label="parent_id">
            <TreeSelect
              treeData={treeSelectOptions}
              placeholder="选择父级分类或留空作为顶级分类"
              allowClear
              treeDefaultExpandAll
            />
          </Form.Item>
          <Form.Item name="description" label="description">
            <Input.TextArea rows={2} />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
