import { useEffect, useMemo, useState } from 'react'
import {
  Button,
  Card,
  Col,
  Drawer,
  Empty,
  Form,
  Input,
  InputNumber,
  Row,
  Space,
  Spin,
  Switch,
  Table,
  Tag,
  Typography,
  message,
} from 'antd'
import type { ColumnsType } from 'antd/es/table'
import type { KnowledgeQueryItem, KnowledgeQueryParams } from '../services/knowledgeQuery'
import { getKnowledgeQueryDefaults, queryKnowledge } from '../services/knowledgeQuery'

const { Paragraph, Text, Title } = Typography

const DEFAULT_PAGE_SIZE = 5
const PAGE_SIZE_OPTIONS = ['5', '10', '20', '50']

interface SearchFormValues extends Required<Omit<KnowledgeQueryParams, 'rerank_id'>> {
  rerank_id?: string
}

function formatScore(value?: number | null): string {
  if (value === undefined || value === null || Number.isNaN(value)) return '—'
  return value.toFixed(4)
}

function formatKeywordTag(value?: string | null): string {
  return value?.trim() || '—'
}

export default function KnowledgeQueryPage() {
  const [form] = Form.useForm<SearchFormValues>()
  const [items, setItems] = useState<KnowledgeQueryItem[]>([])
  const [loading, setLoading] = useState(false)
  const [defaultsLoading, setDefaultsLoading] = useState(true)
  const [total, setTotal] = useState(0)
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(DEFAULT_PAGE_SIZE)
  const [selectedItem, setSelectedItem] = useState<KnowledgeQueryItem | null>(null)
  const [drawerOpen, setDrawerOpen] = useState(false)

  const pageSizeOptions = useMemo(
    () =>
      Array.from(new Set([...PAGE_SIZE_OPTIONS, String(pageSize)])).sort(
        (left, right) => Number(left) - Number(right)
      ),
    [pageSize]
  )

  useEffect(() => {
    let active = true

    void getKnowledgeQueryDefaults()
      .then((defaults) => {
        if (!active) return
        form.setFieldsValue({
          question: '',
          ...defaults,
          rerank_id: defaults.rerank_id ?? '',
        })
        setPage(defaults.page)
        setPageSize(defaults.page_size)
      })
      .catch(() => {
        if (active) message.error('全局检索参数加载失败，请稍后重试')
      })
      .finally(() => {
        if (active) setDefaultsLoading(false)
      })

    return () => {
      active = false
    }
  }, [form])

  const fetchList = async (
    values: SearchFormValues,
    nextPage = values.page,
    nextPageSize = values.page_size
  ) => {
    const params: KnowledgeQueryParams = {
      ...values,
      question: values.question.trim(),
      page: nextPage,
      page_size: nextPageSize,
      rerank_id: values.rerank_id?.trim() ?? '',
    }

    setLoading(true)
    try {
      const res = await queryKnowledge(params)

      setItems(res.items)
      setTotal(res.total)
      setPage(nextPage)
      setPageSize(nextPageSize)
      form.setFieldsValue({ page: nextPage, page_size: nextPageSize })
    } catch {
      message.error('检索失败，请稍后重试')
    } finally {
      setLoading(false)
    }
  }

  const handleTableChange = (newPage: number, newPageSize: number) => {
    void form
      .validateFields()
      .then((values) => fetchList(values, newPage, newPageSize))
      .catch(() => undefined)
  }

  const openDetail = (record: KnowledgeQueryItem) => {
    setSelectedItem(record)
    setDrawerOpen(true)
  }

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
      render: (content: string) => (
        <Paragraph ellipsis={{ rows: 2, expandable: false }}>{content || '—'}</Paragraph>
      ),
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
  ]

  return (
    <div style={{ display: 'grid', gap: 16 }}>
      <Title level={4} style={{ margin: 0 }}>
        知识库查询
      </Title>

      <Card title="1. 检索区域" variant="borderless">
        <Spin spinning={defaultsLoading} tip="正在加载全局配置">
          <Form<SearchFormValues>
            form={form}
            layout="vertical"
            disabled={defaultsLoading}
            onFinish={(values) => void fetchList(values)}
          >
            <Form.Item
              label="问题"
              name="question"
              rules={[{ required: true, whitespace: true, message: '请输入检索内容' }]}
            >
              <Input.TextArea rows={3} placeholder="输入问题或关键词" allowClear />
            </Form.Item>

            <Row gutter={16}>
              <Col xs={24} sm={12} lg={6}>
                <Form.Item
                  label="页码"
                  name="page"
                  rules={[{ required: true, type: 'number', min: 1 }]}
                >
                  <InputNumber min={1} precision={0} style={{ width: '100%' }} />
                </Form.Item>
              </Col>
              <Col xs={24} sm={12} lg={6}>
                <Form.Item
                  label="每页数量"
                  name="page_size"
                  rules={[{ required: true, type: 'number', min: 1, max: 100 }]}
                >
                  <InputNumber min={1} max={100} precision={0} style={{ width: '100%' }} />
                </Form.Item>
              </Col>
              <Col xs={24} sm={12} lg={6}>
                <Form.Item
                  label="相似度阈值"
                  name="similarity_threshold"
                  rules={[{ required: true, type: 'number', min: 0, max: 1 }]}
                >
                  <InputNumber min={0} max={1} step={0.05} style={{ width: '100%' }} />
                </Form.Item>
              </Col>
              <Col xs={24} sm={12} lg={6}>
                <Form.Item
                  label="向量相似度权重"
                  name="vector_similarity_weight"
                  rules={[{ required: true, type: 'number', min: 0, max: 1 }]}
                >
                  <InputNumber min={0} max={1} step={0.05} style={{ width: '100%' }} />
                </Form.Item>
              </Col>
              <Col xs={24} sm={12} lg={6}>
                <Form.Item
                  label="Top K"
                  name="top_k"
                  rules={[{ required: true, type: 'number', min: 1 }]}
                >
                  <InputNumber min={1} precision={0} style={{ width: '100%' }} />
                </Form.Item>
              </Col>
              <Col xs={24} sm={12} lg={6}>
                <Form.Item label="Rerank ID" name="rerank_id">
                  <Input placeholder="留空表示不使用 reranker" allowClear />
                </Form.Item>
              </Col>
              <Col xs={24} sm={12} lg={6}>
                <Form.Item
                  label="请求超时（秒）"
                  name="timeout_secs"
                  rules={[{ required: true, type: 'number', min: 1 }]}
                >
                  <InputNumber min={1} precision={0} style={{ width: '100%' }} />
                </Form.Item>
              </Col>
              <Col xs={12} sm={6} lg={3}>
                <Form.Item label="关键字匹配" name="keyword" valuePropName="checked">
                  <Switch />
                </Form.Item>
              </Col>
              <Col xs={12} sm={6} lg={3}>
                <Form.Item label="返回高亮" name="highlight" valuePropName="checked">
                  <Switch />
                </Form.Item>
              </Col>
            </Row>

            <Button type="primary" htmlType="submit" loading={loading} aria-label="检索">
              检索
            </Button>
          </Form>
        </Spin>
      </Card>

      <Card title="2. 结果区域" variant="borderless">
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
              pageSizeOptions,
              onChange: handleTableChange,
            }}
          />
        )}
      </Card>

      <Drawer
        title={
          selectedItem ? `知识片段明细：${selectedItem.document_keyword ?? ''}` : '知识片段明细'
        }
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
              <Paragraph style={{ marginTop: 6 }}>{selectedItem.highlight || '—'}</Paragraph>
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
                  selectedItem.important_keywords.map((keyword) => (
                    <Tag key={keyword}>{keyword}</Tag>
                  ))
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
  )
}
