import apiClient from './api'
import type { GameTypeItem } from './gameAlias'

export interface StrategyMeta {
  channel: string[]
  client_type: string[]
  strategy: string
}

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

export interface RecommendedGameListResponse {
  items: RecommendedGame[]
  total: number
}

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

export const getRecommendedGame = async (id: number): Promise<RecommendedGame> => {
  const { data } = await apiClient.get(`/recommended-games/${id}`)
  return data
}

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

export const deleteRecommendedGame = async (id: number): Promise<void> => {
  await apiClient.delete(`/recommended-games/${id}`)
}
