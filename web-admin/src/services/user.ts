import apiClient from './api';

export interface UserItem {
  id: number;
  uid: string;
  nickname: string;
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
  /** 用户 ID 精确匹配（数字） */
  id?: number;
  /** 跨 uid / nickname 的模糊匹配关键字 */
  search?: string;
  /** 首次使用时间范围（ISO8601，闭区间） */
  created_at_from?: string;
  created_at_to?: string;
  /** 最近使用时间范围（ISO8601，闭区间） */
  updated_at_from?: string;
  updated_at_to?: string;
}

export const getUsers = async (params: ListUsersParams): Promise<UserListResponse> => {
  const q: Record<string, string> = {
    page: String(params.page),
    page_size: String(params.page_size),
  };
  if (params.id !== undefined) q.id = String(params.id);
  if (params.search) q.search = params.search;
  if (params.created_at_from) q.created_at_from = params.created_at_from;
  if (params.created_at_to)   q.created_at_to   = params.created_at_to;
  if (params.updated_at_from) q.updated_at_from = params.updated_at_from;
  if (params.updated_at_to)   q.updated_at_to   = params.updated_at_to;
  const response = await apiClient.get('/users', { params: q });
  return response.data;
};

export interface CreateUserRequest {
  uid?: string;
  nickname?: string;
}

export const createUser = async (data: CreateUserRequest): Promise<UserItem> => {
  const response = await apiClient.post('/users', data);
  return response.data;
};

export const deleteUser = async (id: number): Promise<void> => {
  await apiClient.delete(`/users/${id}`);
};
