import apiClient from './api';

export interface DashboardStats {
  total_sessions: number;
  total_messages: number;
  total_active_days: number;
  last_session_at: string | null;
}

export async function getStats(): Promise<DashboardStats> {
  const resp = await apiClient.get<DashboardStats>('/users/dashboard/stats');
  return resp.data;
}
