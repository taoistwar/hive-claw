import { useCallback, useEffect, useState } from 'react'
import {
  RuntimeAuditLog,
  RuntimeAuditLogSearchParams,
  getRuntimeAuditLog,
  getRuntimeAuditLogs,
} from '../services/runtimeAuditLog'

const DEFAULT_PAGE_SIZE = 20

export function useRuntimeAuditLogs() {
  const [auditLogs, setAuditLogs] = useState<RuntimeAuditLog[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [pagination, setPagination] = useState({
    current: 1,
    pageSize: DEFAULT_PAGE_SIZE,
    total: 0,
  })
  const [searchParams, setSearchParams] = useState<RuntimeAuditLogSearchParams>({})
  const [selectedLog, setSelectedLog] = useState<RuntimeAuditLog | null>(null)
  const [showDetail, setShowDetail] = useState(false)

  const fetchPage = useCallback(
    async (page: number, pageSize: number, params: RuntimeAuditLogSearchParams) => {
      setLoading(true)
      setError(null)
      try {
        const response = await getRuntimeAuditLogs((page - 1) * pageSize, pageSize, params)
        setAuditLogs(response.items)
        setPagination({ current: page, pageSize, total: response.total })
      } catch {
        setError('获取运行时审计日志列表失败')
      } finally {
        setLoading(false)
      }
    },
    []
  )

  const handleSearch = useCallback(
    async (params: RuntimeAuditLogSearchParams) => {
      setSearchParams(params)
      await fetchPage(1, pagination.pageSize, params)
    },
    [fetchPage, pagination.pageSize]
  )

  const handleReset = useCallback(async () => {
    setSearchParams({})
    await fetchPage(1, pagination.pageSize, {})
  }, [fetchPage, pagination.pageSize])

  const handlePageChange = useCallback(
    async (page: number, pageSize: number) => {
      await fetchPage(page, pageSize, searchParams)
    },
    [fetchPage, searchParams]
  )

  const handleViewDetail = useCallback(async (id: number) => {
    setError(null)
    try {
      setSelectedLog(await getRuntimeAuditLog(id))
      setShowDetail(true)
    } catch {
      setError('获取运行时审计日志详情失败')
    }
  }, [])

  const handleCloseDetail = useCallback(() => {
    setShowDetail(false)
    setSelectedLog(null)
  }, [])

  useEffect(() => {
    void fetchPage(1, DEFAULT_PAGE_SIZE, {})
  }, [fetchPage])

  return {
    auditLogs,
    loading,
    error,
    pagination,
    selectedLog,
    showDetail,
    handleSearch,
    handleReset,
    handlePageChange,
    handleViewDetail,
    handleCloseDetail,
  }
}
