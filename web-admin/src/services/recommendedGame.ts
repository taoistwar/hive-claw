import apiClient from './api'
import type { GameTypeItem } from './gameAlias'

/** @deprecated 推荐游戏管理已废弃，此类型仅用于历史兼容。 */
export interface StrategyMeta {
  channel: string[]
  client_type: string[]
  strategy: string
}

/** @deprecated 推荐游戏管理已废弃，此类型仅用于历史兼容。 */
export interface RecommendedGame {
  id: number
  name: string
  reply: string
  reason: string | null
  tag: string | null
  game_category: GameTypeItem[] | null
  game_image: string | null
  sort_value: number
  game_id: string
  game_name: string
  created_at: string
  updated_at: string
  strategies: StrategyMeta[]
}

/** @deprecated 推荐游戏管理已废弃，此类型仅用于历史兼容。 */
export interface RecommendedGameListResponse {
  items: RecommendedGame[]
  total: number
}

/** @deprecated 推荐游戏管理已废弃，仅为历史管理页保留。 */
export const listRecommendedGames = async (
  q?: string,
  channel?: string,
  client_type?: string,
  page = 1,
  page_size = 20
): Promise<RecommendedGameListResponse> => {
  const params = new URLSearchParams()
  if (q) params.set('q', q)
  if (channel) params.set('channel', channel)
  if (client_type) params.set('client_type', client_type)
  params.set('page', String(page))
  params.set('page_size', String(page_size))
  const { data } = await apiClient.get(`/recommended-games?${params.toString()}`)
  return data
}

/** @deprecated 推荐游戏管理已废弃，仅为历史管理页保留。 */
export const getRecommendedGame = async (id: number): Promise<RecommendedGame> => {
  const { data } = await apiClient.get(`/recommended-games/${id}`)
  return data
}

/** @deprecated 推荐游戏管理已废弃，仅为历史数据维护保留。 */
export const createRecommendedGame = async (meta: {
  name: string
  reply: string
  reason?: string
  tag?: string
  game_category?: GameTypeItem[]
  strategies?: StrategyMeta[]
  game_image?: string
  game_id: string
  game_name: string
}): Promise<RecommendedGame> => {
  const { data } = await apiClient.post('/recommended-games', meta)
  return data
}

/** @deprecated 推荐游戏管理已废弃，仅为历史数据维护保留。 */
export const updateRecommendedGame = async (
  id: number,
  meta: {
    name?: string
    reply?: string
    reason?: string
    tag?: string
    game_category?: GameTypeItem[]
    strategies?: StrategyMeta[]
    game_image?: string
    game_id?: string
    game_name?: string
  }
): Promise<RecommendedGame> => {
  const { data } = await apiClient.put(`/recommended-games/${id}`, meta)
  return data
}

/** @deprecated 推荐游戏管理已废弃，仅为历史数据维护保留。 */
export const deleteRecommendedGame = async (id: number): Promise<void> => {
  await apiClient.delete(`/recommended-games/${id}`)
}

/** @deprecated 推荐游戏管理已废弃，仅为历史管理页保留。 */
export const getExistingGameIds = async (): Promise<string[]> => {
  const { data } = await apiClient.get('/recommended-games/game-ids')
  return data
}
