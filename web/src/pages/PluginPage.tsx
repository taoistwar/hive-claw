// PluginPage — 列表 + 左侧分类树过滤 + 抽屉编辑 + 引用阻塞确认 (T077)

import { useEffect, useState } from 'react';
import { Button, Drawer, Modal, Space, Table, Tag, Tree, message } from 'antd';
import type { ColumnsType } from 'antd/es/table';
import type { DataNode } from 'antd/es/tree';
import { PluginDetail } from '../components/PluginDetail';
import { PluginEdit } from '../components/PluginEdit';
import { PluginFilters } from '../components/PluginFilters';
import { PluginUploader } from '../components/PluginUploader';
import { usePlugins } from '../hooks/usePlugins';
import { deletePlugin, downloadPlugin, type Plugin } from '../services/plugin';
import { listCategoriesTree, type CategoryNode } from '../services/category';

function toDataNodes(nodes: CategoryNode[]): DataNode[] {
  return nodes.map((n) => ({
    key: n.id,
    title: n.name,
    children: n.children?.length ? toDataNodes(n.children) : undefined,
  }));
}

export default function PluginPage() {
  const { items, total, loading, params, setParams, refresh } = usePlugins();
  const [uploadOpen, setUploadOpen] = useState(false);
  const [editOpen, setEditOpen] = useState(false);
  const [viewOpen, setViewOpen] = useState(false);
  const [editingPlugin, setEditingPlugin] = useState<Plugin | null>(null);
  const [viewingPlugin, setViewingPlugin] = useState<Plugin | null>(null);
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

  const handleCategorySelect = (key?: React.Key) => {
    const newSelected = key === selectedCategoryKey ? undefined : key;
    setSelectedCategoryKey(newSelected);
    setParams({ ...params, category_id: newSelected !== undefined ? Number(newSelected) : undefined, offset: 0 });
  };

  const handleDownload = async (p: Plugin) => {
    try {
      await downloadPlugin(p);
    } catch (e) {
      void message.error(`下载失败：${(e as Error).message}`);
    }
  };

  const onDelete = (p: Plugin) => {
    Modal.confirm({
      title: `软删除 Plugin「${p.identifier}@${p.version}」？`,
      content: '若被 Function 引用，将返回错误码 4093 阻止删除。',
      okText: '确认删除',
      cancelText: '取消',
      onOk: async () => {
        try {
          await deletePlugin(p.id);
          void message.success('已软删除');
          await refresh();
        } catch (e: unknown) {
          const err = e as { response?: { data?: { code?: number; message?: string } } };
          const code = err.response?.data?.code;
          if (code === 4093) {
            void message.error(err.response?.data?.message ?? '被引用，无法删除');
          } else {
            void message.error(`删除失败：${(e as Error).message}`);
          }
        }
      },
    });
  };

  const columns: ColumnsType<Plugin> = [
    { title: 'ID', dataIndex: 'id', width: 60 },
    { title: 'identifier', dataIndex: 'identifier' },
    { title: 'name', dataIndex: 'name' },
    { title: 'version', dataIndex: 'version', width: 100 },
    {
      title: 'category',
      dataIndex: 'category_id',
      render: (categoryId: number | null) => (categoryId ? `ID: ${categoryId}` : '—'),
    },
    {
      title: 'tags',
      dataIndex: 'tags',
      render: (tags?: { id: number; name: string }[]) =>
        tags?.map((t) => <Tag key={t.id}>{t.name}</Tag>) ?? null,
    },
    {
      title: 'size',
      dataIndex: 'size_bytes',
      width: 100,
      render: (n: number) => `${(n / 1024).toFixed(1)} KB`,
    },
    {
      title: 'deleted',
      dataIndex: 'deleted_at',
      width: 90,
      render: (v: string | null) => (v ? <Tag color="red">deleted</Tag> : null),
    },
    {
      title: 'actions',
      key: 'actions',
      render: (_, p) => (
        <Space>
          <Button size="small" onClick={() => { setViewingPlugin(p); setViewOpen(true); }}>
            查看
          </Button>
          <Button size="small" onClick={() => { setEditingPlugin(p); setEditOpen(true); }}>
            编辑
          </Button>
          <Button size="small" onClick={() => void handleDownload(p)} disabled={!!p.deleted_at}>
            下载
          </Button>
          <Button danger size="small" onClick={() => onDelete(p)} disabled={!!p.deleted_at}>
            删除
          </Button>
        </Space>
      ),
    },
  ];

  return (
    <div style={{ display: 'flex', gap: 16 }}>
      <div style={{ width: 280, flexShrink: 0, borderRight: '1px solid #f0f0f0', paddingRight: 16 }}>
        <h4 style={{ margin: '0 0 8px 0' }}>Categories</h4>
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
            loadData={async () => {}}
          />
        )}
      </div>
      <div style={{ flex: 1, minWidth: 0 }}>
        <Space style={{ marginBottom: 16 }}>
          <Button type="primary" onClick={() => setUploadOpen(true)}>
            上传 Plugin
          </Button>
        </Space>
        <PluginFilters value={params} onChange={setParams} />
        <Table<Plugin>
          rowKey="id"
          columns={columns}
          dataSource={items}
          loading={loading}
          pagination={{
            current: Math.floor((params.offset ?? 0) / (params.limit ?? 20)) + 1,
            pageSize: params.limit ?? 20,
            total,
            onChange: (page, pageSize) => {
              setParams({ ...params, offset: (page - 1) * pageSize, limit: pageSize });
            },
          }}
          style={{ marginTop: 16 }}
        />
        <Drawer
          title={viewingPlugin ? `查看 Plugin「${viewingPlugin.identifier}@${viewingPlugin.version}」` : '查看 Plugin'}
          open={viewOpen}
          width={600}
          onClose={() => {
            setViewOpen(false);
            setViewingPlugin(null);
          }}
        >
          {viewingPlugin && (
            <PluginDetail
              plugin={viewingPlugin}
              onBack={() => {
                setViewOpen(false);
                setViewingPlugin(null);
              }}
              onEdit={() => {
                setViewOpen(false);
                setEditingPlugin(viewingPlugin);
                setEditOpen(true);
                setViewingPlugin(null);
              }}
            />
          )}
        </Drawer>
        <Drawer
          title="上传 Plugin"
          open={uploadOpen}
          width={520}
          onClose={() => setUploadOpen(false)}
        >
          <PluginUploader
            onUploaded={() => {
              setUploadOpen(false);
              void refresh();
            }}
          />
        </Drawer>
        <Drawer
          title={editingPlugin ? `编辑 Plugin「${editingPlugin.identifier}@${editingPlugin.version}」` : '编辑 Plugin'}
          open={editOpen}
          width={520}
          onClose={() => {
            setEditOpen(false);
            setEditingPlugin(null);
          }}
        >
          {editingPlugin && (
            <PluginEdit
              plugin={editingPlugin}
              onUpdated={() => {
                setEditOpen(false);
                setEditingPlugin(null);
                void refresh();
              }}
              onCancel={() => {
                setEditOpen(false);
                setEditingPlugin(null);
              }}
            />
          )}
        </Drawer>
      </div>
    </div>
  );
}
