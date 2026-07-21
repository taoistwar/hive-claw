import { useCallback, useEffect, useState } from 'react'
import { Alert, Button, Drawer, Form, Input, InputNumber, Modal, Select, Space, Spin, Steps, Table, Tag, message } from 'antd'
import { PlusOutlined, DeleteOutlined } from '@ant-design/icons'
import type { ColumnsType } from 'antd/es/table'
import {
  createRecommendedGame,
  deleteRecommendedGame,
  getExistingGameIds,
  listRecommendedGames,
  updateRecommendedGame,
  type RecommendedGame,
  type StrategyMeta,
} from '../services/recommendedGame'
import { getExternalGames, getExternalGameDetail, type ExternalGameOption, type GameTypeItem } from '../services/gameAlias'

const TAG_OPTIONS = ['运营推荐', '新游上线', '本周热玩']

type PreviewValues = {
  name?: string
  reply?: string
  reason?: string
  tag?: string
  game_category?: GameTypeItem[]
  strategies?: StrategyMeta[]
  game_image?: string
  sort_value?: number
  game_id?: string
  game_name?: string
}

const TAG_COLOR_MAP: Record<string, string> = {
  '运营推荐': '#63b3ed',
  '新游上线': '#48bb78',
  '本周热玩': '#ecc94b',
}

/** @deprecated 推荐游戏管理已废弃，仅为历史管理页保留。 */
function PreviewPanel({ values }: { values: PreviewValues }) {
  const s = (v: unknown): string => (typeof v === 'string' ? v.trim() : '')
  const tag = s(values.tag)
  const name = s(values.name) || '推荐名称'
  const reply = s(values.reply)
  const reason = s(values.reason)
  const image = s(values.game_image)
  const gameName = s(values.game_name) || name
  const categories = values.game_category ?? []

  return (
    <div className="rg-preview">
      <div className="rg-preview__section">
        <div className="rg-preview__title">入口预览</div>
        <div className="rg-preview__hint">用户进入推荐时看到的小条提示</div>
        <div className="rg-preview__entry">
          <span className="rg-preview__entry-tag" style={{ color: tag ? TAG_COLOR_MAP[tag] ?? '#63b3ed' : '#666' }}>
            {tag ? `[${tag}]` : '[未选择标签]'}
          </span>
          <span className="rg-preview__entry-name">{name}</span>
          {reply && <span className="rg-preview__entry-reply">{reply.length > 40 ? `${reply.slice(0, 40)}...` : reply}</span>}
        </div>
      </div>

      <div className="rg-preview__section">
        <div className="rg-preview__title">明细预览</div>
        <div className="rg-preview__hint">用户点击后看到的游戏详情卡片</div>
        <div className="rg-preview__card">
          <div className="rg-preview__cover" style={image ? { backgroundImage: `url(${image})` } : undefined}>
            {!image && <div className="rg-preview__cover-placeholder">推荐图片</div>}
            <div className="rg-preview__online">已上线</div>
            <div className="rg-preview__cover-title">《{gameName}》</div>
          </div>
          <div className="rg-preview__card-body">
            {reason && <div className="rg-preview__desc">{reason}</div>}
            <div className="rg-preview__meta">
              {categories.length > 0 && categories.map((c, i) => (
                <span key={i}>
                  {c.name}{c.type ? ` (${c.type})` : ''}
                  {i < categories.length - 1 && <span className="rg-preview__dot">·</span>}
                </span>
              ))}
            </div>
            <div className="rg-preview__actions">
              <button type="button" className="rg-preview__btn rg-preview__btn--primary">立即云玩</button>
              <button type="button" className="rg-preview__btn">查看详情</button>
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}

/**
 * @deprecated 推荐游戏管理已废弃，仅为查看和维护历史数据保留。
 */
export default function RecommendedGamePage() {
  const [items, setItems] = useState<RecommendedGame[]>([])
  const [loading, setLoading] = useState(false)
  const [total, setTotal] = useState(0)
  const [createOpen, setCreateOpen] = useState(false)
  const [editOpen, setEditOpen] = useState(false)
  const [editingItem, setEditingItem] = useState<RecommendedGame | null>(null)
  const [search, setSearch] = useState('')
  const [channelSearch, setChannelSearch] = useState('')
  const [clientTypeSearch, setClientTypeSearch] = useState('')
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(5)
  const [createForm] = Form.useForm()
  const [editForm] = Form.useForm()
  const [externalGames, setExternalGames] = useState<ExternalGameOption[]>([])
  const [externalLoading, setExternalLoading] = useState(false)
  const [existingGameIds, setExistingGameIds] = useState<Set<string>>(new Set())
  const [availableChannels, setAvailableChannels] = useState<string[]>([])
  const [availableClientTypes, setAvailableClientTypes] = useState<string[]>([])
  const [createStep, setCreateStep] = useState(0)
  const [editStep, setEditStep] = useState(0)

  const createValues = Form.useWatch([], createForm) as PreviewValues | undefined
  const editValues = Form.useWatch([], editForm) as PreviewValues | undefined

  const refresh = useCallback(async () => {
    setLoading(true)
    try {
      const res = await listRecommendedGames(search || undefined, channelSearch || undefined, clientTypeSearch || undefined, page, pageSize)
      setItems(res.items)
      setTotal(res.total)
    } catch (e) {
      void message.error(`加载失败：${(e as Error).message}`)
    } finally {
      setLoading(false)
    }
  }, [search, channelSearch, clientTypeSearch, page, pageSize])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const loadExternalGames = useCallback(async () => {
    setExternalLoading(true)
    try {
      const games = await getExternalGames()
      setExternalGames(games)
    } catch {
      void message.error('加载游戏列表失败')
    } finally {
      setExternalLoading(false)
    }
  }, [])

  const loadExistingIds = useCallback(async () => {
    try {
      const ids = await getExistingGameIds()
      setExistingGameIds(new Set(ids))
    } catch {
      // 获取失败时不阻塞用户操作
    }
  }, [])

  useEffect(() => {
    if (createOpen) {
      createForm.resetFields()
      setCreateStep(0)
    }
    if (editOpen) {
      setEditStep(0)
    }
    if (createOpen || editOpen) {
      void loadExternalGames()
      void loadExistingIds()
    }
  }, [createOpen, editOpen, loadExternalGames, loadExistingIds, createForm])

  const onCreate = async (values: { name: string; reply: string; reason?: string; tag?: string; game_category?: GameTypeItem[]; strategies?: StrategyMeta[]; game_image?: string; sort_value?: number; game_id: string; game_name: string }) => {
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
      strategies: item.strategies.length > 0 ? item.strategies : [{ channel: ['*'], client_type: ['*'], strategy: 'INCLUDE' }],
      game_image: item.game_image,
      sort_value: item.sort_value,
      game_id: item.game_id,
      game_name: item.game_name,
    })
    // 从外部数据库获取可选渠道和客户端类型
    const gid = Number(item.game_id)
    if (!Number.isNaN(gid)) {
      getExternalGameDetail(gid).then((detail) => {
        setAvailableChannels(detail.channels ?? [])
        setAvailableClientTypes(detail.client_types ?? [])
      }).catch(() => { })
    }
    setEditOpen(true)
  }

  const onEdit = async (values: { name: string; reply: string; reason?: string; tag?: string; game_category?: GameTypeItem[]; strategies?: StrategyMeta[]; game_image?: string; sort_value?: number; game_id: string; game_name: string }) => {
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
    return <Tag color={TAG_COLOR_MAP[tag] ? undefined : 'default'} style={TAG_COLOR_MAP[tag] ? { background: `${TAG_COLOR_MAP[tag]}22`, color: TAG_COLOR_MAP[tag], borderColor: `${TAG_COLOR_MAP[tag]}55` } : undefined}>{tag}</Tag>
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
      width: 120,
      render: (cats: GameTypeItem[] | null) => {
        if (!cats || cats.length === 0) return <span style={{ color: '#999' }}>—</span>
        return <span>{cats.map(c => c.name).join(' / ')}</span>
      },
    },
    {
      title: '策略',
      dataIndex: 'strategies',
      width: 200,
      render: (strategies: StrategyMeta[]) => {
        if (!strategies || strategies.length === 0) return <span style={{ color: '#999' }}>—</span>
        return (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
            {strategies.map((s, i) => (
              <span key={i} style={{ fontSize: 12 }}>
                <Tag color={s.strategy === 'EXCLUDE' ? 'red' : 'blue'} style={{ marginRight: 4 }}>{s.strategy}</Tag>
                渠道:{s.channel?.join(',')}
                {' '}客户端:{s.client_type?.join(',')}
              </span>
            ))}
          </div>
        )
      },
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

  const renderFormFields = (isCreate: boolean, form: ReturnType<typeof Form.useForm>[0], step: number) => {
    const handleGameIdChange = async (value: unknown) => {
      const raw = value === undefined || value === null ? '' : String(value)
      if (raw === '') {
        return
      }
      const id = Number(raw)
      const match = externalGames.find((g) => g.id === id)
      if (match) {
        // 立即填充名称、游戏名称和回复模板
        form.setFieldsValue({
          name: `《${match.name}》已上线云玩，想了解吗？`,
          game_name: match.name,
          reply: `为你找到《${match.name}》，海马云电脑已上线，可直接云玩 ✨`,
        })
        // 异步获取详细数据
        try {
          const detail = await getExternalGameDetail(id)
          setAvailableChannels(detail.channels ?? [])
          setAvailableClientTypes(detail.client_types ?? [])
          form.setFieldsValue({
            reason: detail.description ?? undefined,
            game_image: detail.cover_image ?? undefined,
            game_category: detail.game_tags ?? [],
            strategies: [{ channel: ['*'], client_type: ['*'], strategy: 'INCLUDE' }],
          })
        } catch {
          // 详情获取失败，字段留空
        }
      }
    }
    return (
      <>
        <div style={{ display: step === 0 ? 'block' : 'none' }}>
          <Form.Item name="tag" label="标签">
            <Select allowClear placeholder="从三个标签中选择" options={TAG_OPTIONS.map((t) => ({ label: t, value: t }))} />
          </Form.Item>
          <Form.Item name="name" label="名称" rules={[{ required: true, message: '请输入名称' }]}>
            <Input />
          </Form.Item>
          <Form.Item
            name="game_id"
            label="游戏"
            rules={[{ required: true, message: '请选择游戏' }]}
            extra="从外部 cc_logic_game 表中获取"
          >
            <Select
              showSearch
              placeholder="搜索并选择游戏"
              loading={externalLoading}
              notFoundContent={externalLoading ? <Spin size="small" /> : '暂无数据'}
              optionFilterProp="label"
              options={externalGames.map((g) => {
                const gid = String(g.id)
                // 新建时所有已添加的游戏都禁用；编辑时当前游戏不禁用
                const disabled = isCreate
                  ? existingGameIds.has(gid)
                  : existingGameIds.has(gid) && gid !== String(editingItem?.game_id)
                return {
                  value: gid,
                  label: `${g.id} - ${g.name}${disabled ? '（已添加）' : ''}`,
                  disabled,
                }
              })}
              onChange={(value) => {
                const gid = String(value)
                const dup = isCreate
                  ? existingGameIds.has(gid)
                  : existingGameIds.has(gid) && gid !== String(editingItem?.game_id)
                if (dup) {
                  void message.warning('该游戏已添加为推荐游戏')
                  return
                }
                handleGameIdChange(value)
              }}
              filterOption={(input, option) => {
                const label = (option?.label ?? '').toString().toLowerCase()
                return label.includes(input.toLowerCase())
              }}
            />
          </Form.Item>
          <Form.Item name="game_name" label="游戏名称" rules={[{ required: true, message: '请输入游戏名称' }]}>
            <Input placeholder="选择游戏后自动填充，可手动修改" />
          </Form.Item>
          <Form.Item name="reply" label="回复" rules={[{ required: true, message: '请输入回复内容' }]}>
            <Input.TextArea rows={4} />
          </Form.Item>
          <Form.Item name="reason" label="推荐理由（可选）">
            <Input.TextArea rows={3} placeholder="推荐理由" />
          </Form.Item>
          <Form.Item name="game_category" label="游戏类型">
            <Form.List name="game_category">
              {(fields, { add, remove }) => (
                <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                  {fields.map(({ key, name, ...rest }) => (
                    <Space key={key} align="start">
                      <Form.Item {...rest} name={[name, 'name']} noStyle>
                        <Input placeholder="类型名称，如：角色扮演" style={{ width: 160 }} />
                      </Form.Item>
                      <Form.Item {...rest} name={[name, 'type']} noStyle>
                        <Input placeholder="类型标识，如：RPG" style={{ width: 160 }} />
                      </Form.Item>
                      <Button
                        type="text"
                        danger
                        icon={<DeleteOutlined />}
                        onClick={() => remove(name)}
                      />
                    </Space>
                  ))}
                  <Button type="dashed" onClick={() => add({ name: '', type: '' })} block icon={<PlusOutlined />}>
                    添加类型
                  </Button>
                </div>
              )}
            </Form.List>
          </Form.Item>
          <Form.Item name="game_image" label="推荐图片地址">
            <Input placeholder="游戏推荐的图片 URL" />
          </Form.Item>
          <Form.Item name="sort_value" label="排序值" tooltip="数值越大越靠前，默认 0">
            <InputNumber min={0} style={{ width: '100%' }} />
          </Form.Item>
        </div>
        <div style={{ display: step === 1 ? 'block' : 'none' }}>
          <Form.Item label="策略配置">
            <Form.List name="strategies">
              {(fields, { add, remove }) => (
                <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
                  {fields.map(({ key, name, ...rest }) => (
                    <div key={key} style={{ display: 'flex', alignItems: 'center', gap: 8, padding: 8, border: '1px solid #d9d9d9', borderRadius: 6 }}>
                      <Form.Item {...rest} name={[name, 'strategy']} style={{ marginBottom: 0, width: 120 }}>
                        <Select
                          options={[
                            { label: 'INCLUDE', value: 'INCLUDE' },
                            { label: 'EXCLUDE', value: 'EXCLUDE' },
                          ]}
                        />
                      </Form.Item>
                      <Form.Item {...rest} name={[name, 'channel']} style={{ marginBottom: 0, flex: 1, minWidth: 0 }}>
                        <Select
                          mode="multiple"
                          placeholder="渠道"
                          options={[
                            { label: 'ALL', value: '*' },
                            ...availableChannels.map((ch) => ({ label: ch, value: ch })),
                          ]}
                          onChange={(vals: string[]) => {
                            const last = vals[vals.length - 1]
                            if (last === '*') {
                              form.setFieldValue(['strategies', name, 'channel'], ['*'])
                            } else if (vals.includes('*')) {
                              form.setFieldValue(['strategies', name, 'channel'], vals.filter((v: string) => v !== '*'))
                            }
                          }}
                        />
                      </Form.Item>
                      <Form.Item {...rest} name={[name, 'client_type']} style={{ marginBottom: 0, flex: 1, minWidth: 0 }}>
                        <Select
                          mode="multiple"
                          placeholder="客户端"
                          options={[
                            { label: 'ALL', value: '*' },
                            ...availableClientTypes.map((ct) => ({ label: ct, value: ct })),
                          ]}
                          onChange={(vals: string[]) => {
                            const last = vals[vals.length - 1]
                            if (last === '*') {
                              form.setFieldValue(['strategies', name, 'client_type'], ['*'])
                            } else if (vals.includes('*')) {
                              form.setFieldValue(['strategies', name, 'client_type'], vals.filter((v: string) => v !== '*'))
                            }
                          }}
                        />
                      </Form.Item>
                      <Button type="text" danger icon={<DeleteOutlined />} size="small" onClick={() => remove(name)} />
                    </div>
                  ))}
                  <Button type="dashed" onClick={() => add({ channel: ['*'], client_type: ['*'], strategy: 'INCLUDE' })} block icon={<PlusOutlined />}>
                    添加策略
                  </Button>
                </div>
              )}
            </Form.List>
          </Form.Item>
        </div>
      </>
    )
  }

  return (
    <div>
      <Alert
        type="warning"
        showIcon
        message="推荐游戏管理已废弃"
        description="该功能仅为兼容历史数据保留，请勿用于新的业务配置。"
        style={{ marginBottom: 16 }}
      />
      <Space style={{ marginBottom: 16 }}>
        <Button type="primary" onClick={() => { createForm.resetFields(); setCreateOpen(true) }}>
          新建推荐游戏
        </Button>
        <Input.Search
          placeholder="搜索名称/游戏ID/游戏名称"
          allowClear
          onSearch={setSearch}
          style={{ width: 220 }}
          aria-label="搜索推荐游戏"
        />
        <Input
          placeholder="渠道"
          allowClear
          value={channelSearch}
          onChange={(e) => { setChannelSearch(e.target.value); setPage(1) }}
          style={{ width: 120 }}
        />
        <Input
          placeholder="客户端类型"
          allowClear
          value={clientTypeSearch}
          onChange={(e) => { setClientTypeSearch(e.target.value); setPage(1) }}
          style={{ width: 140 }}
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
        onCancel={() => { setCreateOpen(false); setCreateStep(0) }}
        width={960}
        destroyOnHidden
        footer={
          <div style={{ display: 'flex', justifyContent: 'space-between' }}>
            <div>
              {createStep > 0 && (
                <Button htmlType="button" onClick={() => setCreateStep(0)}>上一步</Button>
              )}
            </div>
            <div>
              <Button htmlType="button" onClick={() => { setCreateOpen(false); setCreateStep(0) }} style={{ marginRight: 8 }}>取消</Button>
              {createStep === 0 ? (
                <Button htmlType="button" type="primary" onClick={() => setCreateStep(1)}>下一步</Button>
              ) : (
                <Button type="primary" onClick={async () => {
                  try {
                    const values = await createForm.validateFields()
                    await onCreate(values)
                    setCreateStep(0)
                  } catch { /* validation failed */ }
                }}>提交</Button>
              )}
            </div>
          </div>
        }
      >
        <Steps
          current={createStep}
          size="small"
          style={{ marginBottom: 16 }}
          items={[
            { title: '游戏推荐信息' },
            { title: '策略配置' },
          ]}
        />
        {createStep === 0 ? (
          <div className="rg-modal-split">
            <div className="rg-modal-split__form">
              <Form form={createForm} layout="vertical" initialValues={{ sort_value: 0, tag: '运营推荐' }}>
                {renderFormFields(true, createForm, createStep)}
              </Form>
            </div>
            <div className="rg-modal-split__preview">
              <PreviewPanel values={createValues ?? {}} />
            </div>
          </div>
        ) : (
          <Form form={createForm} layout="vertical" initialValues={{ sort_value: 0, tag: '运营推荐' }}>
            {renderFormFields(true, createForm, createStep)}
          </Form>
        )}
      </Modal>

      <Drawer
        title={editingItem ? `编辑推荐游戏「${editingItem.name}」` : '编辑推荐游戏'}
        open={editOpen}
        width={960}
        onClose={() => {
          setEditOpen(false)
          setEditingItem(null)
          setEditStep(0)
        }}
      >
        {editingItem && (
          <>
            <Steps
              current={editStep}
              size="small"
              style={{ marginBottom: 16 }}
              items={[
                { title: '游戏推荐信息' },
                { title: '策略配置' },
              ]}
            />
            {editStep === 0 ? (
              <div className="rg-modal-split">
                <div className="rg-modal-split__form">
                  <Form form={editForm} layout="vertical">
                    {renderFormFields(false, editForm, editStep)}
                    <Form.Item>
                      <Space>
                        <Button htmlType="button" type="primary" onClick={() => setEditStep(1)}>下一步</Button>
                      </Space>
                    </Form.Item>
                  </Form>
                </div>
                <div className="rg-modal-split__preview">
                  <PreviewPanel values={editValues ?? {}} />
                </div>
              </div>
            ) : (
              <Form form={editForm} layout="vertical">
                {renderFormFields(false, editForm, editStep)}
                <Form.Item>
                  <Space>
                    <Button htmlType="button" onClick={() => setEditStep(0)}>上一步</Button>
                    <Button type="primary" onClick={async () => {
                      try {
                        const values = await editForm.validateFields()
                        await onEdit(values)
                        setEditStep(0)
                      } catch { /* validation failed */ }
                    }}>保存</Button>
                  </Space>
                </Form.Item>
              </Form>
            )}
          </>
        )}
      </Drawer>
    </div>
  )
}
