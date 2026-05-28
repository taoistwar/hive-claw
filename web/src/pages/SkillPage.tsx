// SkillPage — Skill 列表 + 左侧分类树过滤 + 高级搜索过滤 + 创建/编辑（含 SkillMarkdownEditor）

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
  Tag,
  Tree,
  Typography,
  message,
  Row,
  Col,
  DatePicker,
  Card,
} from 'antd';
import type { ColumnsType } from 'antd/es/table';
import type { DataNode } from 'antd/es/tree';
import type { Dayjs } from 'dayjs';
import { SearchOutlined, FilterOutlined, ReloadOutlined } from '@ant-design/icons';

import SkillTestModal from '../components/SkillTestModal';
import { SkillMarkdownEditor } from '../components/SkillMarkdownEditor';
import {
  createSkill,
  deleteSkill,
  listSkills,
  updateSkill,
  type SkillItem,
  type SkillList,
} from '../services/skill';
import { listCategoriesTree, type CategoryNode } from '../services/category';

const { Title, Paragraph } = Typography;
const { RangePicker } = DatePicker;

interface FilterValues {
  search?: string;
  source?: 'workspace' | 'builtin' | '';
  identifier?: string;
  name?: string;
  description?: string;
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

export default function SkillPage() {
  const [data, setData] = useState<SkillList>({ items: [], total: 0, offset: 0, limit: 20 });
  const [loading, setLoading] = useState(false);
  const [categoryId, setCategoryId] = useState<number | undefined>(undefined);
  const [preview, setPreview] = useState<SkillItem | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [editing, setEditing] = useState<SkillItem | null>(null);
  const [form] = Form.useForm();
  const [content, setContent] = useState<string>('');
  const [frontmatter, setFrontmatter] = useState<unknown | null>(null);
  const [testOpen, setTestOpen] = useState(false);
  const [testingSkill, setTestingSkill] = useState<SkillItem | null>(null);
  const [categoryTree, setCategoryTree] = useState<CategoryNode[]>([]);
  const [selectedCategoryKey, setSelectedCategoryKey] = useState<React.Key | undefined>(undefined);
  const [treeExpandedKeys, setTreeExpandedKeys] = useState<React.Key[]>([]);
  const [treeLoading, setTreeLoading] = useState(false);
  const [filterForm] = Form.useForm<FilterValues>();
  const [filterCollapsed, setFilterCollapsed] = useState(false);

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
    setCategoryId(newSelected !== undefined ? Number(newSelected) : undefined);
  };

  const fetchData = (overrides?: Partial<FilterValues>) => {
    const values = overrides || filterForm.getFieldsValue();
    setLoading(true);
    listSkills({
      search: values.search || undefined,
      source: values.source || undefined,
      category_id: categoryId,
      identifier: values.identifier || undefined,
      name: values.name || undefined,
      description: values.description || undefined,
      required_capabilities: values.required_capabilities || undefined,
      created_at_start: values.created_at_range ? values.created_at_range[0].startOf('day').toISOString() : undefined,
      created_at_end: values.created_at_range ? values.created_at_range[1].endOf('day').toISOString() : undefined,
      updated_at_start: values.updated_at_range ? values.updated_at_range[0].startOf('day').toISOString() : undefined,
      updated_at_end: values.updated_at_range ? values.updated_at_range[1].endOf('day').toISOString() : undefined,
    })
      .then(setData)
      .catch((e) => message.error(`加载失败：${(e as Error).message}`))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    fetchData();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [categoryId]);

  const handleFilter = () => {
    fetchData();
  };

  const handleReset = () => {
    filterForm.resetFields();
    fetchData({});
  };

  const openCreate = () => {
    form.resetFields();
    setContent('');
    setFrontmatter(null);
    setCreateOpen(true);
  };

  const openEdit = (s: SkillItem) => {
    form.setFieldsValue({
      identifier: s.identifier,
      name: s.name,
      description: s.description,
    });
    setContent(s.content);
    setFrontmatter(s.frontmatter);
    setEditing(s);
  };

  const onSubmitCreate = async (values: {
    identifier: string;
    name: string;
    description: string;
  }) => {
    if (!content.trim()) {
      void message.error('content 不能为空');
      return;
    }
    try {
      await createSkill({
        identifier: values.identifier,
        name: values.name,
        description: values.description,
        content,
        frontmatter: frontmatter ?? undefined,
      });
      void message.success('已创建');
      setCreateOpen(false);
      await refresh();
    } catch (e: unknown) {
      const err = e as { response?: { data?: { code?: number; message?: string } } };
      void message.error(err.response?.data?.message ?? (e as Error).message);
    }
  };

  const onSubmitEdit = async (values: { name: string; description: string }) => {
    if (!editing) return;
    if (!content.trim()) {
      void message.error('content 不能为空');
      return;
    }
    try {
      await updateSkill(editing.id, {
        name: values.name,
        description: values.description,
        content,
        frontmatter: frontmatter ?? undefined,
        is_always: editing.is_always,
        updated_at: editing.updated_at,
      });
      void message.success('已保存');
      setEditing(null);
      await refresh();
    } catch (e: unknown) {
      const err = e as { response?: { data?: { code?: number; message?: string } } };
      void message.error(err.response?.data?.message ?? (e as Error).message);
    }
  };

  const refresh = useCallback(() => {
    fetchData();
  }, [categoryId]);

  const onDelete = (s: SkillItem) => {
    Modal.confirm({
      title: `删除 Skill「${s.identifier}」？`,
      content:
        s.source === 'builtin'
          ? '内置 Skill 不可删除（5008）'
          : '若被 Agent 引用将返回 4093 阻止',
      okButtonProps: { danger: true },
      onOk: async () => {
        try {
          await deleteSkill(s.id);
          void message.success('已删除');
          await refresh();
        } catch (e: unknown) {
          const err = e as { response?: { data?: { code?: number; message?: string } } };
          void message.error(err.response?.data?.message ?? (e as Error).message);
        }
      },
    });
  };

  const columns: ColumnsType<SkillItem> = [
    { title: 'ID', dataIndex: 'id', width: 60 },
    { title: 'identifier', dataIndex: 'identifier' },
    { title: 'name', dataIndex: 'name' },
    {
      title: 'source',
      dataIndex: 'source',
      width: 110,
      render: (s: string) =>
        s === 'builtin' ? <Tag color="purple">builtin</Tag> : <Tag color="blue">workspace</Tag>,
    },
    {
      title: 'is_always',
      dataIndex: 'is_always',
      width: 100,
      render: (v: boolean) => (
        <Tag color={v ? 'green' : 'default'}>{v ? 'Always' : 'No'}</Tag>
      ),
    },
    {
      title: 'required_capabilities',
      dataIndex: 'required_capabilities',
      width: 200,
      render: (caps?: string[] | null) =>
        caps?.map((c) => <Tag key={c}>{c}</Tag>) ?? '-',
    },
    {
      title: 'size',
      dataIndex: 'content',
      width: 100,
      render: (c: string) => `${(new Blob([c]).size / 1024).toFixed(1)} KB`,
    },
    { title: '创建时间', dataIndex: 'created_at', width: 180, render: (v: string) => new Date(v).toLocaleString() },
    { title: '更新时间', dataIndex: 'updated_at', width: 180, render: (v: string) => new Date(v).toLocaleString() },
    {
      title: 'actions',
      key: 'actions',
      width: 280,
      render: (_, rec) => (
        <Space>
          <Button size="small" onClick={() => setPreview(rec)}>
            预览
          </Button>
          <Button size="small" onClick={() => {
            setTestingSkill(rec);
            setTestOpen(true);
          }}>
            测试
          </Button>
          <Button size="small" onClick={() => openEdit(rec)} disabled={rec.source === 'builtin'}>
            编辑
          </Button>
          <Button size="small" danger onClick={() => onDelete(rec)}>
            删除
          </Button>
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
        <div style={{ marginBottom: 16, display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
          <Space>
            <Button type="primary" onClick={openCreate}>
              新建 Skill
            </Button>
          </Space>
          <Button
            type="link"
            icon={<FilterOutlined />}
            onClick={() => setFilterCollapsed(!filterCollapsed)}
          >
            {filterCollapsed ? '展开过滤' : '收起过滤'}
          </Button>
        </div>

        {!filterCollapsed && (
          <Card style={{ marginBottom: 16 }} size="small">
            <Form form={filterForm} layout="inline">
              <Row gutter={[16, 8]} style={{ width: '100%' }}>
                <Col span={6}>
                  <Form.Item label="搜索" name="search" style={{ width: '100%', marginBottom: 0 }}>
                    <Input placeholder="搜索 name/identifier/description/content" allowClear />
                  </Form.Item>
                </Col>
                <Col span={6}>
                  <Form.Item label="identifier" name="identifier" style={{ width: '100%', marginBottom: 0 }}>
                    <Input placeholder="模糊匹配" allowClear />
                  </Form.Item>
                </Col>
                <Col span={6}>
                  <Form.Item label="name" name="name" style={{ width: '100%', marginBottom: 0 }}>
                    <Input placeholder="模糊匹配" allowClear />
                  </Form.Item>
                </Col>
                <Col span={6}>
                  <Form.Item label="description" name="description" style={{ width: '100%', marginBottom: 0 }}>
                    <Input placeholder="模糊匹配" allowClear />
                  </Form.Item>
                </Col>
                <Col span={6}>
                  <Form.Item label="source" name="source" style={{ width: '100%', marginBottom: 0 }}>
                    <Select
                      allowClear
                      placeholder="全部 source"
                      options={[
                        { value: 'workspace', label: 'workspace' },
                        { value: 'builtin', label: 'builtin' },
                      ]}
                    />
                  </Form.Item>
                </Col>
                <Col span={6}>
                  <Form.Item label="capability" name="required_capabilities" style={{ width: '100%', marginBottom: 0 }}>
                    <Input placeholder="按 capability 名称过滤" allowClear />
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

        <Table<SkillItem>
          rowKey="id"
          columns={columns}
          dataSource={data.items}
          loading={loading}
          pagination={{ total: data.total, pageSize: data.limit }}
        />

        <Drawer
          title={preview ? `Skill: ${preview.identifier}` : ''}
          open={!!preview}
          width={640}
          onClose={() => setPreview(null)}
        >
          {preview && (
            <>
              <Title level={5}>{preview.name}</Title>
              <Paragraph type="secondary">{preview.description}</Paragraph>
              <pre
                style={{
                  background: '#f5f5f5',
                  padding: 12,
                  whiteSpace: 'pre-wrap',
                  fontSize: 13,
                }}
              >
                {preview.content}
              </pre>
            </>
          )}
        </Drawer>

        <Drawer
          title="新建 Skill"
          open={createOpen}
          width={720}
          onClose={() => setCreateOpen(false)}
          destroyOnHidden
        >
          <Form form={form} layout="vertical" onFinish={onSubmitCreate}>
            <Form.Item name="identifier" label="identifier" rules={[{ required: true }]}>
              <Input placeholder="code-review" />
            </Form.Item>
            <Form.Item name="name" label="name" rules={[{ required: true }]}>
              <Input placeholder="Code Review" />
            </Form.Item>
            <Form.Item name="description" label="description" rules={[{ required: true }]}>
              <Input />
            </Form.Item>
            <Form.Item label="content + frontmatter" required>
              <SkillMarkdownEditor
                content={content}
                frontmatter={frontmatter}
                onContentChange={setContent}
                onFrontmatterChange={setFrontmatter}
              />
            </Form.Item>
            <Form.Item>
              <Space>
                <Button type="primary" htmlType="submit">
                  创建
                </Button>
                <Button onClick={() => setCreateOpen(false)}>取消</Button>
              </Space>
            </Form.Item>
          </Form>
        </Drawer>

        <Drawer
          title={editing ? `编辑 Skill — ${editing.identifier}` : ''}
          open={!!editing}
          width={720}
          onClose={() => setEditing(null)}
          destroyOnHidden
        >
          {editing && (
            <Form form={form} layout="vertical" onFinish={onSubmitEdit}>
              <Form.Item label="identifier">
                <Input value={editing.identifier} disabled />
              </Form.Item>
              <Form.Item name="name" label="name" rules={[{ required: true }]}>
                <Input />
              </Form.Item>
              <Form.Item name="description" label="description" rules={[{ required: true }]}>
                <Input />
              </Form.Item>
              <Form.Item label="is_always" valuePropName="checked">
                <Switch checked={editing.is_always} onChange={(checked) => {
                  setEditing({ ...editing, is_always: checked });
                }} />
              </Form.Item>
              <Form.Item label="content + frontmatter" required>
                <SkillMarkdownEditor
                  content={content}
                  frontmatter={frontmatter}
                  onContentChange={setContent}
                  onFrontmatterChange={setFrontmatter}
                />
              </Form.Item>
              <Form.Item>
                <Space>
                  <Button type="primary" htmlType="submit">
                    保存
                  </Button>
                  <Button onClick={() => setEditing(null)}>取消</Button>
                </Space>
              </Form.Item>
            </Form>
          )}
        </Drawer>

        <SkillTestModal
          visible={testOpen}
          skillId={testingSkill?.id ?? null}
          skillName={testingSkill?.identifier ?? ''}
          onCancel={() => setTestOpen(false)}
        />
      </div>
    </div>
  );
}
