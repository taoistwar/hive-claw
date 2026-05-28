import { useState, useCallback, useEffect } from 'react';
import {
  AdminAuditLog,
  getAdminAuditLogs,
  getAdminAuditLog,
  AdminAuditLogSearchParams,
} from '../services/adminAuditLog';

export const useAdminAuditLogs = () => {
  const [auditLogs, setAuditLogs] = useState<AdminAuditLog[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pagination, setPagination] = useState({
    current: 1,
    pageSize: 10,
    total: 0,
  });

  const [searchParams, setSearchParams] = useState<AdminAuditLogSearchParams>({});
  const [selectedLog, setSelectedLog] = useState<AdminAuditLog | null>(null);
  const [showDetail, setShowDetail] = useState(false);

  const doFetch = useCallback(async (page: number, params: AdminAuditLogSearchParams) => {
    setLoading(true);
    setError(null);
    try {
      const offset = (page - 1) * pagination.pageSize;
      const response = await getAdminAuditLogs(offset, pagination.pageSize, params);
      setAuditLogs(response.items);
      setPagination({
        current: page,
        pageSize: pagination.pageSize,
        total: response.total,
      });
    } catch (err) {
      setError('获取管理审计日志列表失败');
    } finally {
      setLoading(false);
    }
  }, [pagination.pageSize]);

  const fetchAuditLogs = useCallback(async (page?: number) => {
    await doFetch(page || pagination.current, searchParams);
  }, [pagination.current, searchParams, doFetch]);

  const handleSearch = useCallback(async (params: AdminAuditLogSearchParams) => {
    setSearchParams(params);
    setPagination({ ...pagination, current: 1 });
    await doFetch(1, params);
  }, [pagination, doFetch]);

  const handleReset = useCallback(async () => {
    const emptyParams: AdminAuditLogSearchParams = {};
    setSearchParams(emptyParams);
    setPagination({ ...pagination, current: 1 });
    await doFetch(1, emptyParams);
  }, [pagination, doFetch]);

  const handleViewDetail = useCallback(async (id: number) => {
    try {
      const log = await getAdminAuditLog(id);
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
