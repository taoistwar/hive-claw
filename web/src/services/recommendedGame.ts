import apiClient from './api'

export interface RecommendedGame {
  id: number
  name: string
  reply: string
  reason: string | null
  tag: string | null
  game_category: string | null
  game_image: string | null
  sort_value: number
  game_id: string
  game_name: string
  created_at: string
  updated_at: string
}

export interface RecommendedGameListResponse {
  items: RecommendedGame[]
  total: number
}

export const listRecommendedGames = async (
  q?: string,
  page = 1,
  page_size = 20
): Promise<RecommendedGameListResponse> => {
  const params = new URLSearchParams()
  if (q) params.set('q', q)
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
  game_category?: string
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
    game_category?: string
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
