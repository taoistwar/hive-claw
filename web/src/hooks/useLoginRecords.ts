import { useState, useCallback } from 'react';
import { message } from 'antd';
import {
  LoginRecord,
  getLoginRecords,
  getLoginRecord,
  LoginRecordSearchParams,
} from '../services/loginRecord';

const DEFAULT_PAGE_SIZE = 5;

export const useLoginRecords = () => {
  const [loginRecords, setLoginRecords] = useState<LoginRecord[]>([]);
  const [loading, setLoading] = useState(false);
  const [pagination, setPagination] = useState({
    current: 1,
    pageSize: DEFAULT_PAGE_SIZE,
    total: 0,
  });
  const [searchParams, setSearchParams] = useState<LoginRecordSearchParams>({});
  const [selectedRecord, setSelectedRecord] = useState<LoginRecord | null>(null);
  const [detailModalVisible, setDetailModalVisible] = useState(false);

  const doFetch = useCallback(async (page: number, params: LoginRecordSearchParams) => {
    setLoading(true);
    try {
      const offset = (page - 1) * pagination.pageSize;
      const response = await getLoginRecords(offset, pagination.pageSize, params);
      setLoginRecords(response.items);
      setPagination((prev) => ({ ...prev, current: page, total: response.total }));
    } catch (error: any) {
      message.error(error.response?.data?.message || '获取登录日志列表失败');
    } finally {
      setLoading(false);
    }
  }, [pagination.pageSize]);

  const fetchLoginRecords = useCallback(async (page?: number) => {
    const currentPage = page ?? pagination.current;
    doFetch(currentPage, searchParams);
  }, [pagination.current, searchParams, doFetch]);

  const handleSearch = useCallback(async (params: LoginRecordSearchParams) => {
    setPagination((prev) => ({ ...prev, current: 1 }));
    doFetch(1, params);
  }, [doFetch]);

  const handleReset = useCallback(() => {
    setSearchParams({});
    setPagination((prev) => ({ ...prev, current: 1 }));
    doFetch(1, {});
  }, [doFetch]);

  const viewDetail = useCallback(async (id: number) => {
    try {
      const record = await getLoginRecord(id);
      setSelectedRecord(record);
      setDetailModalVisible(true);
    } catch (error: any) {
      message.error(error.response?.data?.message || '获取登录日志详情失败');
    }
  }, []);

  const closeDetailModal = useCallback(() => {
    setDetailModalVisible(false);
    setSelectedRecord(null);
  }, []);

  return {
    loginRecords,
    loading,
    pagination,
    searchParams,
    selectedRecord,
    detailModalVisible,
    fetchLoginRecords,
    setPagination,
    setSearchParams,
    handleSearch,
    handleReset,
    viewDetail,
    closeDetailModal,
  };
};
