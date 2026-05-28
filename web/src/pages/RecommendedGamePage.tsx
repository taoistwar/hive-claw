import { useCallback, useEffect, useState } from 'react'
import { Button, Drawer, Form, Input, InputNumber, Modal, Select, Space, Table, Tag, message } from 'antd'
import type { ColumnsType } from 'antd/es/table'
import {
  createRecommendedGame,
  deleteRecommendedGame,
  listRecommendedGames,
  updateRecommendedGame,
  type RecommendedGame,
} from '../services/recommendedGame'

const TAG_OPTIONS = ['运营推荐', '新游上线', '本周热玩']

export default function RecommendedGamePage() {
  const [items, setItems] = useState<RecommendedGame[]>([])
  const [loading, setLoading] = useState(false)
  const [total, setTotal] = useState(0)
  const [createOpen, setCreateOpen] = useState(false)
  const [editOpen, setEditOpen] = useState(false)
  const [editingItem, setEditingItem] = useState<RecommendedGame | null>(null)
  const [search, setSearch] = useState('')
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(20)
  const [createForm] = Form.useForm()
  const [editForm] = Form.useForm()

  const refresh = useCallback(async () => {
    setLoading(true)
    try {
      const res = await listRecommendedGames(search || undefined, page, pageSize)
      setItems(res.items)
      setTotal(res.total)
    } catch (e) {
      void message.error(`加载失败：${(e as Error).message}`)
    } finally {
      setLoading(false)
    }
  }, [search, page, pageSize])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const onCreate = async (values: { name: string; reply: string; reason?: string; tag?: string; game_category?: string; game_image?: string; sort_value?: number; game_id: string; game_name: string }) => {
    try {
      await createRecommendedGame(values)
      void message.success('已创建')
      createForm.resetFields()
      setCreateOpen(false)
      await refresh()
    } catch (e) {
      void message.error(`创建失败：${(e as Error).message}`)
    }
  }

  const openEdit = (item: RecommendedGame) => {
    setEditingItem(item)
    editForm.setFieldsValue({
      name: item.name,
      reply: item.reply,
      reason: item.reason,
      tag: item.tag,
      game_category: item.game_category,
      game_image: item.game_image,
      sort_value: item.sort_value,
      game_id: item.game_id,
      game_name: item.game_name,
    })
    setEditOpen(true)
  }

  const onEdit = async (values: { name: string; reply: string; reason?: string; tag?: string; game_category?: string; game_image?: string; sort_value?: number; game_id: string; game_name: string }) => {
    if (!editingItem) return
    try {
      await updateRecommendedGame(editingItem.id, values)
      void message.success('更新成功')
      editForm.resetFields()
      setEditOpen(false)
      setEditingItem(null)
      await refresh()
    } catch (e) {
      void message.error(`更新失败：${(e as Error).message}`)
    }
  }

  const onDelete = (item: RecommendedGame) => {
    Modal.confirm({
      title: `删除推荐游戏「${item.name}」？`,
      okButtonProps: { danger: true },
      onOk: async () => {
        try {
          await deleteRecommendedGame(item.id)
          void message.success('已删除')
          await refresh()
        } catch (e) {
          void message.error(`删除失败：${(e as Error).message}`)
        }
      },
    })
  }

  const renderTag = (tag: string | null) => {
    if (!tag) return <span style={{ color: '#999' }}>—</span>
    const colorMap: Record<string, string> = {
      '运营推荐': 'blue',
      '新游上线': 'green',
      '本周热玩': 'orange',
    }
    return <Tag color={colorMap[tag] ?? 'default'}>{tag}</Tag>
  }

  const columns: ColumnsType<RecommendedGame> = [
    { title: 'ID', dataIndex: 'id', width: 60 },
    { title: '名称', dataIndex: 'name', ellipsis: true },
    {
      title: '回复',
      dataIndex: 'reply',
      ellipsis: true,
      render: (text: string) => <span title={text}>{text.length > 50 ? `${text.slice(0, 50)}...` : text}</span>,
    },
    {
      title: '推荐理由',
      dataIndex: 'reason',
      ellipsis: true,
      render: (text: string | null) => text ? <span title={text}>{text.length > 30 ? `${text.slice(0, 30)}...` : text}</span> : <span style={{ color: '#999' }}>—</span>,
    },
    {
      title: '标签',
      dataIndex: 'tag',
      width: 110,
      render: (tag: string | null) => renderTag(tag),
    },
    {
      title: '游戏类型',
      dataIndex: 'game_category',
      width: 100,
      render: (text: string | null) => text || <span style={{ color: '#999' }}>—</span>,
    },
    {
      title: '排序值',
      dataIndex: 'sort_value',
      width: 90,
      sorter: (a, b) => b.sort_value - a.sort_value,
      defaultSortOrder: 'descend',
    },
    { title: '游戏ID', dataIndex: 'game_id', width: 120 },
    { title: '游戏名称', dataIndex: 'game_name', width: 150 },
    {
      title: '操作',
      key: 'actions',
      width: 150,
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
  ]

  const renderFormFields = (isCreate: boolean) => {
    return (
      <>
        <Form.Item name="name" label="名称" rules={[{ required: true, message: '请输入名称' }]}>
          <Input />
        </Form.Item>
        <Form.Item name="reply" label="回复" rules={[{ required: true, message: '请输入回复内容' }]}>
          <Input.TextArea rows={4} />
        </Form.Item>
        <Form.Item name="reason" label="推荐理由（可选）">
          <Input.TextArea rows={3} placeholder="推荐理由" />
        </Form.Item>
        <Form.Item name="tag" label="标签">
          <Select allowClear placeholder="从三个标签中选择" options={TAG_OPTIONS.map((t) => ({ label: t, value: t }))} />
        </Form.Item>
        <Form.Item name="game_category" label="游戏类型">
          <Input placeholder="如：角色扮演、策略等" />
        </Form.Item>
        <Form.Item name="game_image" label="推荐图片地址">
          <Input placeholder="游戏推荐的图片 URL" />
        </Form.Item>
        <Form.Item name="sort_value" label="排序值" tooltip="数值越大越靠前，默认 0">
          <InputNumber min={0} style={{ width: '100%' }} />
        </Form.Item>
        <Form.Item name="game_id" label="游戏ID" rules={[{ required: true, message: '请输入游戏ID' }]}>
          <Input />
        </Form.Item>
        <Form.Item name="game_name" label="游戏名称" rules={[{ required: true, message: '请输入游戏名称' }]}>
          <Input />
        </Form.Item>
      </>
    )
  }

  return (
    <div>
      <Space style={{ marginBottom: 16 }}>
        <Button type="primary" onClick={() => setCreateOpen(true)}>
          新建推荐游戏
        </Button>
        <Input.Search
          placeholder="搜索名称/游戏ID/游戏名称"
          allowClear
          onSearch={setSearch}
          style={{ width: 280 }}
          aria-label="搜索推荐游戏"
        />
      </Space>
      <Table<RecommendedGame>
        rowKey="id"
        columns={columns}
        dataSource={items}
        loading={loading}
        pagination={{
          current: page,
          pageSize,
          total,
          showSizeChanger: true,
          showTotal: (t) => `共 ${t} 条`,
          onChange: (p, ps) => {
            setPage(p)
            setPageSize(ps)
          },
        }}
      />

      <Modal
        title="新建推荐游戏"
        open={createOpen}
        onCancel={() => setCreateOpen(false)}
        onOk={() => createForm.submit()}
        destroyOnHidden
      >
        <Form form={createForm} layout="vertical" onFinish={onCreate} initialValues={{ sort_value: 0 }}>
          {renderFormFields(true)}
        </Form>
      </Modal>

      <Drawer
        title={editingItem ? `编辑推荐游戏「${editingItem.name}」` : '编辑推荐游戏'}
        open={editOpen}
        width={500}
        onClose={() => {
          setEditOpen(false)
          setEditingItem(null)
        }}
      >
        {editingItem && (
          <Form form={editForm} layout="vertical" onFinish={onEdit}>
            {renderFormFields(false)}
            <Form.Item>
              <Button type="primary" htmlType="submit">
                保存
              </Button>
            </Form.Item>
          </Form>
        )}
      </Drawer>
    </div>
  )
}
