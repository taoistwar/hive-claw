import apiClient from './api'

export interface Game {
  id: number
  name: string
  aliases: string[]
  created_at: string
  updated_at: string
}

export interface GameListResponse {
  total: number
  games: Game[]
}

export interface GameListParams {
  page?: number
  page_size?: number
  q?: string
}

export interface CreateGameRequest {
  name: string
  aliases: string[]
}

export interface UpdateGameRequest {
  name?: string
  aliases?: string[]
}

export interface ExternalGameOption {
  id: number
  name: string
}

export interface ExternalGameDetail {
  id: number
  name: string
  description?: string | null
  cover_image?: string | null
  game_tags?: GameTypeItem[] | null
  client_types?: string[] | null
  channels: string[]
}

export interface GameTypeItem {
  name: string
  type: string
}

const EXTERNAL_GAMES_CACHE_KEY = 'external_games_cache'
const EXTERNAL_GAMES_CACHE_TTL = 30 * 60 * 1000 // 30 分钟

interface CacheEntry {
  data: ExternalGameOption[]
  timestamp: number
}

export const getExternalGames = async (): Promise<ExternalGameOption[]> => {
  // 1. 检查 localStorage 缓存（30 分钟有效）
  try {
    const cached = localStorage.getItem(EXTERNAL_GAMES_CACHE_KEY)
    if (cached) {
      const entry: CacheEntry = JSON.parse(cached)
      if (Date.now() - entry.timestamp < EXTERNAL_GAMES_CACHE_TTL) {
        return entry.data
      }
    }
  } catch { /* 缓存解析失败，走网络请求 */ }

  // 2. 缓存过期或不存在，请求后端
  const { data } = await apiClient.get<ExternalGameOption[]>('/external-games')

  // 3. 写入 localStorage 缓存
  try {
    localStorage.setItem(
      EXTERNAL_GAMES_CACHE_KEY,
      JSON.stringify({ data, timestamp: Date.now() })
    )
  } catch { /* 存储空间不足，忽略 */ }

  return data
}

export const getExternalGameDetail = async (id: number): Promise<ExternalGameDetail> => {
  const { data } = await apiClient.get<ExternalGameDetail>(`/external-games/${id}`)
  return data
}

export const getGames = async (params: GameListParams = {}): Promise<GameListResponse> => {
  const { page = 1, page_size = 10, q } = params
  const searchParams = new URLSearchParams()
  searchParams.set('page', String(page))
  searchParams.set('page_size', String(page_size))
  if (q) searchParams.set('q', q)
  const { data } = await apiClient.get(`/game-aliases?${searchParams.toString()}`)
  return data
}

export const getGameById = async (id: number): Promise<Game> => {
  const { data } = await apiClient.get(`/game-aliases/${id}`)
  return data
}

export const createGame = async (req: CreateGameRequest): Promise<Game> => {
  const { data } = await apiClient.post('/game-aliases', req)
  return data
}

export const updateGame = async (id: number, req: UpdateGameRequest): Promise<Game> => {
  const { data } = await apiClient.put(`/game-aliases/${id}`, req)
  return data
}

export const deleteGame = async (id: number): Promise<void> => {
  await apiClient.delete(`/game-aliases/${id}`)
}
