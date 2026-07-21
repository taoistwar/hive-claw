import React, { useEffect, useState, useCallback } from 'react'
import { Alert, Button, message } from 'antd'
import { PlusOutlined } from '@ant-design/icons'
import GameTable from '../components/GameTable'
import GameAliasForm from '../components/GameAliasForm'
import { useGameAlias } from '../hooks/useGameAlias'
import { deleteGame as apiDeleteGame, type Game } from '../services/gameAlias'
import { useAuth } from '../hooks/useAuth'

/**
 * @deprecated 游戏别名管理已废弃，仅为查看和维护历史数据保留。
 */
const GameAliasPage: React.FC = () => {
  const { admin } = useAuth()
  const canWrite = admin?.role === 2 || admin?.role === 3

  const {
    games,
    loading,
    total,
    page,
    pageSize,
    searchText,
    fetchGames,
    handleSearch,
    handlePageChange,
  } = useGameAlias()

  const [formVisible, setFormVisible] = useState(false)
  const [editingGame, setEditingGame] = useState<Game | null>(null)

  useEffect(() => {
    fetchGames()
  }, [fetchGames])

  const handleRefresh = useCallback(() => {
    fetchGames()
  }, [fetchGames])

  const handleEdit = (game: Game) => {
    setEditingGame(game)
    setFormVisible(true)
  }

  const handleCreate = () => {
    setEditingGame(null)
    setFormVisible(true)
  }

  const handleFormCancel = () => {
    setFormVisible(false)
    setEditingGame(null)
  }

  const handleFormSuccess = () => {
    setFormVisible(false)
    setEditingGame(null)
    handleRefresh()
  }

  const handleDelete = async (id: number) => {
    try {
      await apiDeleteGame(id)
      message.success('删除成功')
      handleRefresh()
    } catch {
      message.error('删除失败')
    }
  }

  return (
    <div>
      <Alert
        type="warning"
        showIcon
        message="游戏别名管理已废弃"
        description="该功能仅为兼容历史数据保留，请勿用于新的业务配置。"
        style={{ marginBottom: 16 }}
      />
      <div style={{ marginBottom: 16, display: 'flex', justifyContent: 'flex-end' }}>
        {canWrite && (
          <Button type="primary" icon={<PlusOutlined />} onClick={handleCreate}>
            添加游戏别名
          </Button>
        )}
      </div>
      <GameTable
        games={games}
        loading={loading}
        total={total}
        page={page}
        pageSize={pageSize}
        searchText={searchText}
        canWrite={canWrite}
        onSearch={handleSearch}
        onPageChange={handlePageChange}
        onEdit={handleEdit}
        onDelete={handleDelete}
      />
      <GameAliasForm
        visible={formVisible}
        editingGame={editingGame}
        onCancel={handleFormCancel}
        onSuccess={handleFormSuccess}
      />
    </div>
  )
}

export default GameAliasPage
