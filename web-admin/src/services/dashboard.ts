import apiClient from './api';

export interface DashboardStats {
  totalAdmins: number;
  activeAdmins: number;
  todayLogins: number;
  disabledAdmins: number;
}

export interface LoginRecord {
  id: number;
  admin_id: number;
  admin_nickname: string;
  login_at: string;
  ip_address: string;
  success: boolean;
}

export const getDashboardStats = async (): Promise<DashboardStats> => {
  const response = await apiClient.get('/dashboard/stats');
  return response.data;
};

export const getRecentLogins = async (limit: number): Promise<LoginRecord[]> => {
  const response = await apiClient.get('/dashboard/recent-logins', {
    params: { limit },
  });
  return response.data;
};
