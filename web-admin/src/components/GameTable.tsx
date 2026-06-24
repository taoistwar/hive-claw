import React from 'react'
import { Table, Tag, Button, Space, Input, Popconfirm } from 'antd'
import { EditOutlined, DeleteOutlined } from '@ant-design/icons'
import type { ColumnsType } from 'antd/es/table'
import type { Game } from '../services/gameAlias'

const { Search } = Input

interface GameTableProps {
  games: Game[]
  loading: boolean
  total: number
  page: number
  pageSize: number
  searchText: string
  canWrite: boolean
  onSearch: (value: string) => void
  onPageChange: (page: number, pageSize: number) => void
  onEdit: (game: Game) => void
  onDelete: (id: number) => void
}

const GameTable: React.FC<GameTableProps> = ({
  games,
  loading,
  total,
  page,
  pageSize,
  searchText,
  canWrite,
  onSearch,
  onPageChange,
  onEdit,
  onDelete,
}) => {
  const columns: ColumnsType<Game> = [
    {
      title: 'ID',
      dataIndex: 'id',
      key: 'id',
      width: 80,
    },
    {
      title: '名称',
      dataIndex: 'name',
      key: 'name',
      ellipsis: true,
    },
    {
      title: '别名',
      dataIndex: 'aliases',
      key: 'aliases',
      render: (aliases: string[]) => (
        <div style={{ display: 'flex', flexWrap: 'wrap', gap: '4px' }}>
          {aliases.map((alias) => (
            <Tag key={alias} color="blue">
              {alias}
            </Tag>
          ))}
          {aliases.length === 0 && <span style={{ color: 'var(--text-secondary)' }}>无</span>}
        </div>
      ),
    },
    {
      title: '创建日期',
      dataIndex: 'created_at',
      key: 'created_at',
      width: 180,
      render: (val: string) => new Date(val).toLocaleString(),
    },
    {
      title: '修改时间',
      dataIndex: 'updated_at',
      key: 'updated_at',
      width: 180,
      render: (val: string) => new Date(val).toLocaleString(),
    },
    {
      title: '操作',
      key: 'actions',
      width: 120,
      render: (_, record) =>
        canWrite ? (
          <Space>
            <Button
              type="link"
              size="small"
              icon={<EditOutlined />}
              onClick={() => onEdit(record)}
            />
            <Popconfirm
              title="确认删除"
              description="删除后不可恢复，确认删除该游戏别名？"
              onConfirm={() => onDelete(record.id)}
              okText="确认"
              cancelText="取消"
            >
              <Button type="link" size="small" danger icon={<DeleteOutlined />} />
            </Popconfirm>
          </Space>
        ) : null,
    },
  ]

  return (
    <div>
      <div style={{ marginBottom: 16 }}>
        <Search
          placeholder="搜索游戏名称或别名"
          value={searchText}
          onChange={(e) => onSearch(e.target.value)}
          onSearch={onSearch}
          style={{ width: 300 }}
          allowClear
        />
      </div>
      <Table<Game>
        columns={columns}
        dataSource={games}
        rowKey="id"
        loading={loading}
        pagination={{
          current: page,
          pageSize,
          total,
          showSizeChanger: true,
          showTotal: (t) => `共 ${t} 条`,
        }}
        onChange={(pagination) => {
          onPageChange(pagination.current ?? 1, pagination.pageSize ?? pageSize)
        }}
      />
    </div>
  )
}

export default GameTable
