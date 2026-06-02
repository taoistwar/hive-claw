import { useState, useCallback } from 'react'
import { message } from 'antd'
import { getGames, type Game, type GameListResponse } from '../services/gameAlias'

export interface UseGameAliasReturn {
  games: Game[]
  loading: boolean
  total: number
  page: number
  pageSize: number
  searchText: string
  fetchGames: (params?: { page?: number; page_size?: number; q?: string }) => Promise<void>
  handleSearch: (value: string) => void
  handlePageChange: (page: number, pageSize: number) => void
}

export function useGameAlias(): UseGameAliasReturn {
  const [games, setGames] = useState<Game[]>([])
  const [loading, setLoading] = useState(false)
  const [total, setTotal] = useState(0)
  const [page, setPage] = useState(1)
  const [pageSize, setPageSize] = useState(10)
  const [searchText, setSearchText] = useState('')

  const fetchGames = useCallback(async (params?: { page?: number; page_size?: number; q?: string }) => {
    setLoading(true)
    try {
      const currentPage = params?.page ?? page
      const currentPageSize = params?.page_size ?? pageSize
      const currentSearch = params?.q !== undefined ? params.q : searchText
      const result: GameListResponse = await getGames({
        page: currentPage,
        page_size: currentPageSize,
        q: currentSearch || undefined,
      })
      setGames(result.games)
      setTotal(result.total)
      setPage(currentPage)
      setPageSize(currentPageSize)
    } catch {
      message.error('加载游戏别名列表失败')
    } finally {
      setLoading(false)
    }
  }, [page, pageSize, searchText])

  const handleSearch = useCallback((value: string) => {
    setSearchText(value)
    setPage(1)
    getGames({ page: 1, page_size: pageSize, q: value || undefined })
      .then((result) => {
        setGames(result.games)
        setTotal(result.total)
      })
      .catch(() => {
        message.error('搜索失败')
      })
  }, [pageSize])

  const handlePageChange = useCallback((newPage: number, newPageSize: number) => {
    setPage(newPage)
    setPageSize(newPageSize)
    getGames({ page: newPage, page_size: newPageSize, q: searchText || undefined })
      .then((result) => {
        setGames(result.games)
        setTotal(result.total)
      })
      .catch(() => {
        message.error('加载失败')
      })
  }, [searchText])

  return {
    games,
    loading,
    total,
    page,
    pageSize,
    searchText,
    fetchGames,
    handleSearch,
    handlePageChange,
  }
}
