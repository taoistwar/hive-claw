import React, { useEffect, useState, useCallback, useMemo } from 'react'
import { Modal, Form, Select, AutoComplete, Input, message, Spin } from 'antd'
import type { Game, CreateGameRequest, UpdateGameRequest, ExternalGameOption } from '../services/gameAlias'
import { createGame, updateGame, getExternalGames } from '../services/gameAlias'

interface GameAliasFormProps {
  visible: boolean
  editingGame: Game | null
  onCancel: () => void
  onSuccess: () => void
}

const MAX_ALIAS_LENGTH = 50
const MAX_ALIASES_COUNT = 20

/**
 * @deprecated 游戏别名管理已废弃，仅为历史数据维护保留。
 */
const GameAliasForm: React.FC<GameAliasFormProps> = ({
  visible,
  editingGame,
  onCancel,
  onSuccess,
}) => {
  const [form] = Form.useForm()
  const [submitting, setSubmitting] = useState(false)
  const [externalGames, setExternalGames] = useState<ExternalGameOption[]>([])
  const [externalGamesLoading, setExternalGamesLoading] = useState(false)
  const isEdit = !!editingGame

  const loadExternalGames = useCallback(async () => {
    setExternalGamesLoading(true)
    try {
      const games = await getExternalGames()
      setExternalGames(games)
    } catch {
      message.error('加载游戏列表失败')
    } finally {
      setExternalGamesLoading(false)
    }
  }, [])

  // 用 useMemo 合并缓存列表 + 编辑中的游戏名，避免在 useEffect 中 setState 触发无限循环
  const gameOptions = useMemo(() => {
    const base = externalGames.map((g) => ({ value: g.name, label: `${g.id} - ${g.name}` }))
    if (editingGame?.name && !base.some((o) => o.value === editingGame.name)) {
      base.push({ value: editingGame.name, label: `${editingGame.id} - ${editingGame.name}` })
    }
    return base
  }, [externalGames, editingGame])

  useEffect(() => {
    if (visible) {
      if (editingGame) {
        form.setFieldsValue({
          name: editingGame.name,
          aliases: editingGame.aliases,
        })
      } else {
        form.resetFields()
        loadExternalGames()
      }
    }
  }, [visible, editingGame, form, loadExternalGames])

  const handleOk = async () => {
    try {
      const values = await form.validateFields()
      setSubmitting(true)

      const aliases: string[] = values.aliases as string[]
      const deduplicatedAliases = [...new Set(aliases.filter((a) => a.trim()))]

      if (isEdit && editingGame) {
        const req: UpdateGameRequest = {
          name: values.name !== editingGame.name ? values.name : undefined,
          aliases: JSON.stringify(deduplicatedAliases) !== JSON.stringify(editingGame.aliases)
            ? deduplicatedAliases
            : undefined,
        }
        if (!req.name && !req.aliases) {
          message.info('没有修改内容')
          setSubmitting(false)
          return
        }
        await updateGame(editingGame.id, req)
        message.success('修改成功')
      } else {
        const req: CreateGameRequest = {
          name: values.name.trim(),
          aliases: deduplicatedAliases,
        }
        await createGame(req)
        message.success('添加成功')
      }
      onSuccess()
      form.resetFields()
    } catch {
      // form validation error or API error (handled by interceptor)
    } finally {
      setSubmitting(false)
    }
  }

  const handleAliasChange = (values: string[]) => {
    const trimmed = values.map((v) => v.trim()).filter((v) => v)
    const unique = [...new Set(trimmed)]
    form.setFieldValue('aliases', unique)
  }

  return (
    <Modal
      title={isEdit ? '编辑游戏别名' : '添加游戏别名'}
      open={visible}
      onOk={handleOk}
      onCancel={onCancel}
      confirmLoading={submitting}
      okText={isEdit ? '保存' : '添加'}
      cancelText="取消"
      width={600}
    >
      <Form
        form={form}
        layout="vertical"
        initialValues={{ name: '', aliases: [] }}
      >
        <Form.Item
          name="name"
          label="游戏名称"
          rules={[
            { required: true, message: '请输入游戏名称' },
          ]}
        >
          <AutoComplete
            placeholder="输入游戏名称搜索"
            disabled={isEdit}
            options={gameOptions}
            filterOption={(input, option) => {
              const label = (option?.label ?? '').toLowerCase()
              const match = label.includes(input.toLowerCase())
              if (!match) return false
              // 未输入时默认最多显示 10 个
              if (!input) {
                const idx = gameOptions.findIndex((o) => o.value === option?.value)
                return idx < 10
              }
              return true
            }}
            notFoundContent={externalGamesLoading ? <Spin size="small" /> : '暂无数据'}
            defaultActiveFirstOption={false}
            style={{ width: '100%' }}
          >
            <Input placeholder="输入游戏名称搜索" autoFocus={!isEdit} />
          </AutoComplete>
        </Form.Item>
        <Form.Item
          name="aliases"
          label="别名"
          rules={[
            { required: true, message: '请至少添加一个别名' },
            {
              validator: (_, value: string[]) => {
                if (!value || value.filter((a: string) => a.trim()).length === 0) {
                  return Promise.reject(new Error('请至少添加一个别名'))
                }
                if (value.length > MAX_ALIASES_COUNT) {
                  return Promise.reject(new Error(`别名数量不能超过 ${MAX_ALIASES_COUNT} 个`))
                }
                for (const alias of value) {
                  if (alias.trim().length > MAX_ALIAS_LENGTH) {
                    return Promise.reject(new Error(`别名不能超过 ${MAX_ALIAS_LENGTH} 个字符`))
                  }
                }
                return Promise.resolve()
              },
            },
          ]}
        >
          <Select
            mode="tags"
            placeholder="输入别名后按回车添加"
            onChange={handleAliasChange}
            tokenSeparators={[',']}
          />
        </Form.Item>
      </Form>
    </Modal>
  )
}

export default GameAliasForm
