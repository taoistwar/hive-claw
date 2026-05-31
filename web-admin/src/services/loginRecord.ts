import apiClient from './api';

export interface LoginRecord {
  id: number;
  admin_id: number | null;
  admin_phone_snapshot: string;
  admin_nickname_snapshot: string;
  login_at: string;
  ip_address: string;
  success: boolean;
  failure_reason: string | null;
}

export interface LoginRecordListResponse {
  items: LoginRecord[];
  total: number;
  offset: number;
  limit: number;
}

export interface LoginRecordSearchParams {
  admin_id?: number;
  search?: string;
  success?: boolean;
  failure_reason?: string;
  ip_address?: string;
  login_at_start?: string;
  login_at_end?: string;
}

export const getLoginRecords = async (
  offset: number,
  limit: number,
  params?: LoginRecordSearchParams
): Promise<LoginRecordListResponse> => {
  const response = await apiClient.get('/login-records', {
    params: { offset, limit, ...params },
  });
  return response.data;
};

export const getLoginRecord = async (id: number): Promise<LoginRecord> => {
  const response = await apiClient.get(`/login-records/${id}`);
  return response.data;
};
