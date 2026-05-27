import { useState, useCallback } from 'react';
import { message } from 'antd';
import {
  ToolItem,
  CreateTool,
  UpdateTool,
  listTools,
  createTool,
  updateTool,
  deleteTool,
  ToolSearchParams,
} from '../services/tool';

const DEFAULT_PAGE_SIZE = 10;

interface UseToolReturn {
  tools: ToolItem[];
  loading: boolean;
  pagination: { current: number; pageSize: number; total: number };
  modalVisible: boolean;
  detailVisible: boolean;
  editingTool: ToolItem | null;
  viewingTool: ToolItem | null;
  searchParams: ToolSearchParams;
  fetchTools: (page?: number) => Promise<void>;
  handleCreate: (data: CreateTool) => Promise<void>;
  handleUpdate: (data: UpdateTool) => Promise<void>;
  handleDelete: (id: number) => Promise<void>;
  openCreateModal: () => void;
  openEditModal: (tool: ToolItem) => void;
  openDetailModal: (tool: ToolItem) => void;
  closeModal: () => void;
  closeDetailModal: () => void;
  setPagination: React.Dispatch<React.SetStateAction<{ current: number; pageSize: number; total: number }>>;
  setSearchParams: React.Dispatch<React.SetStateAction<ToolSearchParams>>;
  handleSearch: (params: ToolSearchParams) => Promise<void>;
  handleReset: () => void;
}

export const useTool = (): UseToolReturn => {
  const [tools, setTools] = useState<ToolItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [pagination, setPagination] = useState({
    current: 1,
    pageSize: DEFAULT_PAGE_SIZE,
    total: 0,
  });
  const [modalVisible, setModalVisible] = useState(false);
  const [detailVisible, setDetailVisible] = useState(false);
  const [editingTool, setEditingTool] = useState<ToolItem | null>(null);
  const [viewingTool, setViewingTool] = useState<ToolItem | null>(null);
  const [searchParams, setSearchParams] = useState<ToolSearchParams>({});

  const doFetch = useCallback(async (page: number, params: ToolSearchParams) => {
    setLoading(true);
    try {
      const offset = (page - 1) * pagination.pageSize;
      const response = await listTools({ ...params, offset, limit: pagination.pageSize });
      setTools(response.items);
      setPagination((prev) => ({ ...prev, current: page, total: response.total }));
    } catch (error: any) {
      message.error(error.response?.data?.message || '获取工具列表失败');
    } finally {
      setLoading(false);
    }
  }, [pagination.pageSize]);

  const fetchTools = useCallback(async (page?: number) => {
    const currentPage = page ?? pagination.current;
    doFetch(currentPage, searchParams);
  }, [pagination.current, searchParams, doFetch]);

  const handleSearch = useCallback(async (params: ToolSearchParams) => {
    setPagination((prev) => ({ ...prev, current: 1 }));
    doFetch(1, params);
  }, [doFetch]);

  const handleReset = useCallback(() => {
    setSearchParams({});
    setPagination((prev) => ({ ...prev, current: 1 }));
    doFetch(1, {});
  }, [doFetch]);

  const handleCreate = async (data: CreateTool) => {
    try {
      await createTool(data);
      message.success('添加工具成功');
      setModalVisible(false);
      doFetch(pagination.current, searchParams);
    } catch (error: any) {
      message.error(error.response?.data?.message || '添加工具失败');
      throw error;
    }
  };

  const handleUpdate = async (data: UpdateTool) => {
    if (!editingTool) return;
    try {
      await updateTool(editingTool.id, data);
      message.success('更新工具成功');
      setModalVisible(false);
      setEditingTool(null);
      doFetch(pagination.current, searchParams);
    } catch (error: any) {
      message.error(error.response?.data?.message || '更新工具失败');
      throw error;
    }
  };

  const handleDelete = async (id: number) => {
    try {
      await deleteTool(id);
      message.success('删除工具成功');
      doFetch(pagination.current, searchParams);
    } catch (error: any) {
      message.error(error.response?.data?.message || '删除工具失败');
    }
  };

  const openCreateModal = () => {
    setEditingTool(null);
    setModalVisible(true);
  };

  const openEditModal = (tool: ToolItem) => {
    setEditingTool(tool);
    setModalVisible(true);
  };

  const openDetailModal = (tool: ToolItem) => {
    setViewingTool(tool);
    setDetailVisible(true);
  };

  const closeModal = () => {
    setModalVisible(false);
    setEditingTool(null);
  };

  const closeDetailModal = () => {
    setDetailVisible(false);
    setViewingTool(null);
  };

  return {
    tools,
    loading,
    pagination,
    modalVisible,
    detailVisible,
    editingTool,
    viewingTool,
    searchParams,
    fetchTools,
    handleCreate,
    handleUpdate,
    handleDelete,
    openCreateModal,
    openEditModal,
    openDetailModal,
    closeModal,
    closeDetailModal,
    setPagination,
    setSearchParams,
    handleSearch,
    handleReset,
  };
};
