import apiClient from './api';

export interface UserItem {
  id: number;
  phone: string;
  status: number;
  created_at: string;
  updated_at: string;
}

export interface UserListResponse {
  users: UserItem[];
  total: number;
  page: number;
  page_size: number;
}

export interface ListUsersParams {
  page: number;
  page_size: number;
  search?: string;
}

export const getUsers = async (params: ListUsersParams): Promise<UserListResponse> => {
  const response = await apiClient.get('/users', { params });
  return response.data;
};

export interface CreateUserRequest {
  phone: string;
  password: string;
}

export const createUser = async (data: CreateUserRequest): Promise<UserItem> => {
  const response = await apiClient.post('/users', data);
  return response.data;
};

export const deleteUser = async (id: number): Promise<void> => {
  await apiClient.delete(`/users/${id}`);
};

export interface ToggleUserStatusRequest {
  status: number;
}

export const toggleUserStatus = async (id: number, data: ToggleUserStatusRequest): Promise<UserItem> => {
  const response = await apiClient.patch(`/users/${id}/status`, data);
  return response.data;
};
