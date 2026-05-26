import { useState, useCallback } from 'react';
import { message } from 'antd';
import {
  Admin,
  getAdmins,
  createAdmin,
  updateAdmin,
  deleteAdmin,
  toggleAdminStatus,
  CreateAdminData,
  UpdateAdminData,
} from '../services/admin';

const DEFAULT_PAGE_SIZE = 10;

interface UseAdminReturn {
  admins: Admin[];
  loading: boolean;
  pagination: { current: number; pageSize: number; total: number };
  modalVisible: boolean;
  editingAdmin: Admin | null;
  fetchAdmins: (page?: number) => Promise<void>;
  handleCreate: (data: CreateAdminData) => Promise<void>;
  handleUpdate: (data: UpdateAdminData) => Promise<void>;
  handleDelete: (id: number) => Promise<void>;
  handleToggleStatus: (id: number, status: number) => Promise<void>;
  openCreateModal: () => void;
  openEditModal: (admin: Admin) => void;
  closeModal: () => void;
  setPagination: React.Dispatch<
    React.SetStateAction<{ current: number; pageSize: number; total: number }>
  >;
}

export const useAdmin = (): UseAdminReturn => {
  const [admins, setAdmins] = useState<Admin[]>([]);
  const [loading, setLoading] = useState(false);
  const [pagination, setPagination] = useState({
    current: 1,
    pageSize: DEFAULT_PAGE_SIZE,
    total: 0,
  });
  const [modalVisible, setModalVisible] = useState(false);
  const [editingAdmin, setEditingAdmin] = useState<Admin | null>(null);

  const fetchAdmins = useCallback(async (page?: number) => {
    const currentPage = page ?? pagination.current;
    setLoading(true);
    try {
      const offset = (currentPage - 1) * pagination.pageSize;
      const response = await getAdmins(offset, pagination.pageSize);
      setAdmins(response.admins);
      setPagination((prev) => ({ ...prev, current: currentPage, total: response.total }));
    } catch (error: any) {
      message.error(error.response?.data?.message || '获取管理员列表失败');
    } finally {
      setLoading(false);
    }
  }, [pagination.pageSize, pagination.current]);

  const handleCreate = async (data: CreateAdminData) => {
    try {
      await createAdmin(data);
      message.success('添加管理员成功');
      setModalVisible(false);
      fetchAdmins();
    } catch (error: any) {
      message.error(error.response?.data?.message || '添加管理员失败');
      throw error;
    }
  };

  const handleUpdate = async (data: UpdateAdminData) => {
    if (!editingAdmin) return;
    try {
      await updateAdmin(editingAdmin.id, data);
      message.success('更新管理员成功');
      setModalVisible(false);
      setEditingAdmin(null);
      fetchAdmins();
    } catch (error: any) {
      message.error(error.response?.data?.message || '更新管理员失败');
      throw error;
    }
  };

  const handleDelete = async (id: number) => {
    try {
      await deleteAdmin(id);
      message.success('删除管理员成功');
      fetchAdmins();
    } catch (error: any) {
      message.error(error.response?.data?.message || '删除管理员失败');
    }
  };

  const handleToggleStatus = async (id: number, status: number) => {
    try {
      await toggleAdminStatus(id, status);
      message.success(status === 1 ? '已启用' : '已禁用');
      fetchAdmins();
    } catch (error: any) {
      message.error(error.response?.data?.message || '操作失败');
    }
  };

  const openCreateModal = () => {
    setEditingAdmin(null);
    setModalVisible(true);
  };

  const openEditModal = (admin: Admin) => {
    setEditingAdmin(admin);
    setModalVisible(true);
  };

  const closeModal = () => {
    setModalVisible(false);
    setEditingAdmin(null);
  };

  return {
    admins,
    loading,
    pagination,
    modalVisible,
    editingAdmin,
    fetchAdmins,
    handleCreate,
    handleUpdate,
    handleDelete,
    handleToggleStatus,
    openCreateModal,
    openEditModal,
    closeModal,
    setPagination,
  };
};
