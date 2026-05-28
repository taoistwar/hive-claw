import { useState, useCallback, useEffect } from 'react';
import {
  RuntimeAuditLog,
  getRuntimeAuditLogs,
  getRuntimeAuditLog,
  RuntimeAuditLogSearchParams,
} from '../services/runtimeAuditLog';

export const useRuntimeAuditLogs = () => {
  const [auditLogs, setAuditLogs] = useState<RuntimeAuditLog[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pagination, setPagination] = useState({
    current: 1,
    pageSize: 10,
    total: 0,
  });

  const [searchParams, setSearchParams] = useState<RuntimeAuditLogSearchParams>({});
  const [selectedLog, setSelectedLog] = useState<RuntimeAuditLog | null>(null);
  const [showDetail, setShowDetail] = useState(false);

  const doFetch = useCallback(async (page: number, params: RuntimeAuditLogSearchParams) => {
    setLoading(true);
    setError(null);
    try {
      const offset = (page - 1) * pagination.pageSize;
      const response = await getRuntimeAuditLogs(offset, pagination.pageSize, params);
      setAuditLogs(response.items);
      setPagination({
        current: page,
        pageSize: pagination.pageSize,
        total: response.total,
      });
    } catch (err) {
      setError('获取运行时审计日志列表失败');
    } finally {
      setLoading(false);
    }
  }, [pagination.pageSize]);

  const fetchAuditLogs = useCallback(async (page?: number) => {
    await doFetch(page || pagination.current, searchParams);
  }, [pagination.current, searchParams, doFetch]);

  const handleSearch = useCallback(async (params: RuntimeAuditLogSearchParams) => {
    setSearchParams(params);
    setPagination({ ...pagination, current: 1 });
    await doFetch(1, params);
  }, [pagination, doFetch]);

  const handleReset = useCallback(async () => {
    const emptyParams: RuntimeAuditLogSearchParams = {};
    setSearchParams(emptyParams);
    setPagination({ ...pagination, current: 1 });
    await doFetch(1, emptyParams);
  }, [pagination, doFetch]);

  const handleViewDetail = useCallback(async (id: number) => {
    try {
      const log = await getRuntimeAuditLog(id);
      setSelectedLog(log);
      setShowDetail(true);
    } catch (err) {
      setError('获取日志详情失败');
    }
  }, []);

  const handleCloseDetail = useCallback(() => {
    setShowDetail(false);
    setSelectedLog(null);
  }, []);

  useEffect(() => {
    fetchAuditLogs();
  }, []);

  return {
    auditLogs,
    loading,
    error,
    pagination,
    searchParams,
    selectedLog,
    showDetail,
    fetchAuditLogs,
    handleSearch,
    handleReset,
    handleViewDetail,
    handleCloseDetail,
    setPagination,
  };
};
