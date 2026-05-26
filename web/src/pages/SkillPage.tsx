// SkillPage — Skill 列表（markdown content + frontmatter）(T094)

import { useEffect, useState } from 'react';
import { Table, Tag, Drawer, Typography, message, Space, Input, Select } from 'antd';
import type { ColumnsType } from 'antd/es/table';
import { listSkills, type SkillItem, type SkillList } from '../services/skill';

const { Title, Paragraph } = Typography;

export default function SkillPage() {
  const [data, setData] = useState<SkillList>({ items: [], total: 0, offset: 0, limit: 20 });
  const [loading, setLoading] = useState(false);
  const [search, setSearch] = useState('');
  const [source, setSource] = useState<'workspace' | 'builtin' | ''>('');
  const [preview, setPreview] = useState<SkillItem | null>(null);

  useEffect(() => {
    setLoading(true);
    listSkills({
      search: search || undefined,
      source: source || undefined,
    })
      .then(setData)
      .catch((e) => message.error(`加载失败：${(e as Error).message}`))
      .finally(() => setLoading(false));
  }, [search, source]);

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
  ];

  return (
    <div>
      <Space style={{ marginBottom: 16 }}>
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
        onRow={(rec) => ({
          onClick: () => setPreview(rec),
          style: { cursor: 'pointer' },
        })}
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
    </div>
  );
}
