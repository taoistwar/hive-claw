/// <reference types="vitest/globals" />
import { render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import GameAliasPage from '../GameAliasPage'
import RecommendedGamePage from '../RecommendedGamePage'

const fetchGames = vi.fn()

vi.mock('../../hooks/useAuth', () => ({
  useAuth: () => ({ admin: { role: 3 } }),
}))

vi.mock('../../hooks/useGameAlias', () => ({
  useGameAlias: () => ({
    games: [],
    loading: false,
    total: 0,
    page: 1,
    pageSize: 10,
    searchText: '',
    fetchGames,
    handleSearch: vi.fn(),
    handlePageChange: vi.fn(),
  }),
}))

vi.mock('../../components/GameTable', () => ({
  default: () => <div data-testid="game-alias-table" />,
}))

vi.mock('../../components/GameAliasForm', () => ({
  default: () => null,
}))

vi.mock('../../services/recommendedGame', () => ({
  createRecommendedGame: vi.fn(),
  deleteRecommendedGame: vi.fn(),
  getExistingGameIds: vi.fn().mockResolvedValue([]),
  listRecommendedGames: vi.fn().mockResolvedValue({ items: [], total: 6 }),
  updateRecommendedGame: vi.fn(),
}))

vi.mock('../../services/gameAlias', () => ({
  deleteGame: vi.fn(),
  getExternalGames: vi.fn().mockResolvedValue([]),
  getExternalGameDetail: vi.fn(),
}))

describe('已废弃业务页面', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('在推荐游戏页面显示废弃提示', async () => {
    render(<RecommendedGamePage />)

    await screen.findByText('共 6 条')
    expect(screen.getByText('推荐游戏管理已废弃')).toBeInTheDocument()
    expect(
      screen.getByText('该功能仅为兼容历史数据保留，请勿用于新的业务配置。')
    ).toBeInTheDocument()
  })

  it('在游戏别名页面显示废弃提示', () => {
    render(<GameAliasPage />)

    expect(screen.getByText('游戏别名管理已废弃')).toBeInTheDocument()
    expect(
      screen.getByText('该功能仅为兼容历史数据保留，请勿用于新的业务配置。')
    ).toBeInTheDocument()
  })
})
