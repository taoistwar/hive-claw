import apiClient from './api';

export interface Admin {
  id: number;
  phone: string;
  nickname: string;
  role: number;
  status: number;
  created_at: string;
  updated_at: string;
  last_login_at: string | null;
}

export interface AdminListResponse {
  items: Admin[];
  total: number;
}

export interface CreateAdminData {
  phone: string;
  nickname: string;
  password: string;
  role: number;
  status: number;
}

export interface UpdateAdminData {
  nickname: string;
  role: number;
  status: number;
}

export interface AdminSearchParams {
  search?: string;
  status?: number;
  role?: number;
  created_at_start?: string;
  created_at_end?: string;
  last_login_start?: string;
  last_login_end?: string;
}

export const getAdmins = async (
  offset: number,
  limit: number,
  params?: AdminSearchParams
): Promise<AdminListResponse> => {
  const response = await apiClient.get('/admins', {
    params: { offset, limit, ...params },
  });
  return response.data;
};

export const getAdmin = async (id: number): Promise<Admin> => {
  const response = await apiClient.get(`/admins/${id}`);
  return response.data;
};

export const createAdmin = async (data: CreateAdminData): Promise<Admin> => {
  const response = await apiClient.post('/admins', data);
  return response.data;
};

export const updateAdmin = async (
  id: number,
  data: UpdateAdminData
): Promise<Admin> => {
  const response = await apiClient.put(`/admins/${id}`, data);
  return response.data;
};

export const deleteAdmin = async (id: number): Promise<void> => {
  await apiClient.delete(`/admins/${id}`);
};

export const toggleAdminStatus = async (
  id: number,
  status: number
): Promise<Admin> => {
  const response = await apiClient.patch(`/admins/${id}/status`, { status });
  return response.data;
};
