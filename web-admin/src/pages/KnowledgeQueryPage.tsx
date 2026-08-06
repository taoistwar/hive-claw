import { useState } from 'react';
import {
  Button,
  Card,
  Drawer,
  Empty,
  Input,
  Space,
  Table,
  Tag,
  Typography,
  message,
} from 'antd';
import type { ColumnsType } from 'antd/es/table';
import type { KnowledgeQueryItem } from '../services/knowledgeQuery';
import { queryKnowledge } from '../services/knowledgeQuery';

const { Search } = Input;
const { Paragraph, Text, Title } = Typography;

const DEFAULT_PAGE_SIZE = 5;
const PAGE_SIZE_OPTIONS = ['5', '10', '20', '50'];

function formatScore(value?: number | null): string {
  if (value === undefined || value === null || Number.isNaN(value)) return '—';
  return value.toFixed(4);
}

function formatKeywordTag(value?: string | null): string {
  return value?.trim() || '—';
}

export default function KnowledgeQueryPage() {
  const [question, setQuestion] = useState('');
  const [items, setItems] = useState<KnowledgeQueryItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(DEFAULT_PAGE_SIZE);
  const [selectedItem, setSelectedItem] = useState<KnowledgeQueryItem | null>(null);
  const [drawerOpen, setDrawerOpen] = useState(false);

  const fetchList = async (nextPage = page, nextPageSize = pageSize) => {
    const q = question.trim();
    if (!q) {
      message.warning('请输入检索内容');
      return;
    }

    setLoading(true);
    try {
      const res = await queryKnowledge({
        question: q,
        page: nextPage,
        page_size: nextPageSize,
      });

      setItems(res.items);
      setTotal(res.total);
      setPage(nextPage);
      setPageSize(nextPageSize);
    } catch {
      message.error('检索失败，请稍后重试');
    } finally {
      setLoading(false);
    }
  };

  const onSearch = () => {
    setPage(1);
    void fetchList(1, pageSize);
  };

  const handleTableChange = (newPage: number, newPageSize: number) => {
    void fetchList(newPage, newPageSize);
  };

  const openDetail = (record: KnowledgeQueryItem) => {
    setSelectedItem(record);
    setDrawerOpen(true);
  };

  const columns: ColumnsType<KnowledgeQueryItem> = [
    {
      title: '序号',
      key: 'index',
      width: 80,
      render: (_, __, index) => <Text>{(page - 1) * pageSize + index + 1}</Text>,
    },
    {
      title: '内容摘要',
      dataIndex: 'content',
      key: 'content',
      render: (content: string) => <Paragraph ellipsis={{ rows: 2, expandable: false }}>{content || '—'}</Paragraph>,
    },
    {
      title: '文档',
      dataIndex: 'document_keyword',
      key: 'document_keyword',
      width: 220,
      render: (v) => <Text>{formatKeywordTag(v)}</Text>,
    },
    {
      title: '数据集',
      dataIndex: 'dataset_id',
      key: 'dataset_id',
      width: 220,
      render: (v) => <Text code>{formatKeywordTag(v)}</Text>,
    },
    {
      title: '匹配分数',
      key: 'scores',
      width: 130,
      render: (_, record) => (
        <Space size={4} direction="vertical">
          <Tag color="blue">sim: {formatScore(record.similarity)}</Tag>
          <Tag color="green">term: {formatScore(record.term_similarity)}</Tag>
          <Tag color="purple">vec: {formatScore(record.vector_similarity)}</Tag>
        </Space>
      ),
    },
    {
      title: '操作',
      key: 'action',
      width: 120,
      render: (_, record) => (
        <Button size="small" type="link" onClick={() => openDetail(record)}>
          查看明细
        </Button>
      ),
    },
  ];

  return (
    <div style={{ display: 'grid', gap: 16 }}>
      <Title level={4} style={{ margin: 0 }}>
        知识库查询
      </Title>

      <Card title="1. 检索区域" bordered={false}>
        <Search
          value={question}
          onChange={(e) => setQuestion(e.target.value)}
          onSearch={onSearch}
          placeholder="输入问题或关键词"
          enterButton="检索"
          allowClear
          size="large"
        />
      </Card>

      <Card title="2. 结果区域" bordered={false}>
        {items.length === 0 && !loading ? (
          <Empty description="暂无结果，请先检索" image={Empty.PRESENTED_IMAGE_SIMPLE} />
        ) : (
          <Table
            rowKey="id"
            columns={columns}
            loading={loading}
            dataSource={items}
            pagination={{
              current: page,
              pageSize,
              total,
              showTotal: (t) => `共 ${t} 条`,
              showSizeChanger: true,
              pageSizeOptions: PAGE_SIZE_OPTIONS,
              onChange: handleTableChange,
            }}
          />
        )}
      </Card>

      <Drawer
        title={selectedItem ? `知识片段明细：${selectedItem.document_keyword ?? ''}` : '知识片段明细'}
        width={560}
        open={drawerOpen}
        onClose={() => setDrawerOpen(false)}
        destroyOnClose
      >
        {selectedItem && (
          <Space direction="vertical" size={14} style={{ width: '100%' }}>
            <div>
              <Text strong>内容</Text>
              <Paragraph style={{ marginTop: 6, whiteSpace: 'pre-wrap' }}>
                {selectedItem.content}
              </Paragraph>
            </div>

            <div>
              <Text strong>高亮片段</Text>
              <Paragraph style={{ marginTop: 6 }}>
                {selectedItem.highlight || '—'}
              </Paragraph>
            </div>

            <div>
              <Text strong>文档 ID</Text>
              <Paragraph copyable={{ text: selectedItem.document_id ?? '' }}>
                {selectedItem.document_id || '—'}
              </Paragraph>
            </div>

            <div>
              <Text strong>数据集 ID</Text>
              <Paragraph code>{selectedItem.dataset_id || '—'}</Paragraph>
            </div>

            <div>
              <Text strong>语义分数</Text>
              <Space style={{ marginTop: 6 }}>
                <Tag color="blue">{formatScore(selectedItem.similarity)}</Tag>
                <Tag color="green">{formatScore(selectedItem.term_similarity)}</Tag>
                <Tag color="purple">{formatScore(selectedItem.vector_similarity)}</Tag>
              </Space>
            </div>

            <div>
              <Text strong>关键字</Text>
              <div style={{ marginTop: 6 }}>
                {selectedItem.important_keywords?.length ? (
                  selectedItem.important_keywords.map((keyword) => <Tag key={keyword}>{keyword}</Tag>)
                ) : (
                  <Text type="secondary">—</Text>
                )}
              </div>
            </div>

            {selectedItem.image_id && (
              <div>
                <Text strong>图片 ID</Text>
                <Paragraph>{selectedItem.image_id}</Paragraph>
              </div>
            )}
          </Space>
        )}
      </Drawer>
    </div>
  );
}
