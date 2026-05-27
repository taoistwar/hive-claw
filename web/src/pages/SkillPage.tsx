// SkillPage — Skill 列表 + 创建/编辑（含 SkillMarkdownEditor）(T094 + T091 integration)

import { useCallback, useEffect, useState } from 'react';
import {
  Button,
  Drawer,
  Form,
  Input,
  Modal,
  Select,
  Space,
  Table,
  Tag,
  Typography,
  message,
} from 'antd';
import type { ColumnsType } from 'antd/es/table';

import { SkillMarkdownEditor } from '../components/SkillMarkdownEditor';
import {
  createSkill,
  deleteSkill,
  listSkills,
  updateSkill,
  type SkillItem,
  type SkillList,
} from '../services/skill';

const { Title, Paragraph } = Typography;

export default function SkillPage() {
  const [data, setData] = useState<SkillList>({ items: [], total: 0, offset: 0, limit: 20 });
  const [loading, setLoading] = useState(false);
  const [search, setSearch] = useState('');
  const [source, setSource] = useState<'workspace' | 'builtin' | ''>('');
  const [preview, setPreview] = useState<SkillItem | null>(null);
  const [createOpen, setCreateOpen] = useState(false);
  const [editing, setEditing] = useState<SkillItem | null>(null);
  const [form] = Form.useForm();
  const [content, setContent] = useState<string>('');
  const [frontmatter, setFrontmatter] = useState<unknown | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const d = await listSkills({
        search: search || undefined,
        source: source || undefined,
      });
      setData(d);
    } catch (e) {
      void message.error(`加载失败：${(e as Error).message}`);
    } finally {
      setLoading(false);
    }
  }, [search, source]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

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
      title: 'size',
      dataIndex: 'content',
      width: 100,
      render: (c: string) => `${(new Blob([c]).size / 1024).toFixed(1)} KB`,
    },
    {
      title: 'actions',
      key: 'actions',
      width: 200,
      render: (_, rec) => (
        <Space>
          <Button size="small" onClick={() => setPreview(rec)}>
            预览
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
    <div>
      <Space style={{ marginBottom: 16 }}>
        <Button type="primary" onClick={openCreate}>
          新建 Skill
        </Button>
        <Input.Search
          placeholder="搜索 skill (name/identifier/content)"
          allowClear
          onSearch={setSearch}
          style={{ width: 320 }}
          aria-label="搜索 skill"
        />
        <Select
          value={source}
          onChange={(v) => setSource(v)}
          style={{ width: 140 }}
          aria-label="按 source 筛选"
          options={[
            { value: '', label: '全部 source' },
            { value: 'workspace', label: 'workspace' },
            { value: 'builtin', label: 'builtin' },
          ]}
        />
      </Space>
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
        destroyOnClose
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
        destroyOnClose
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
    </div>
  );
}
