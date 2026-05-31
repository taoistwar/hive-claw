import apiClient from './api';

export interface AdminAuditLog {
  id: number;
  operator_id: number | null;
  operator_phone_snapshot: string;
  target_admin_id: number | null;
  target_phone_snapshot: string;
  operation: string;
  detail: Record<string, unknown> | null;
  occurred_at: string;
}

export interface AdminAuditLogListResponse {
  items: AdminAuditLog[];
  total: number;
  offset: number;
  limit: number;
}

export interface AdminAuditLogSearchParams {
  operation?: string;
  operator_id?: number;
  target_admin_id?: number;
  occurred_at_start?: string;
  occurred_at_end?: string;
}

export const getAdminAuditLogs = async (
  offset: number = 0,
  limit: number = 20,
  params?: AdminAuditLogSearchParams
): Promise<AdminAuditLogListResponse> => {
  const response = await apiClient.get('/admin-audit-logs', {
    params: { offset, limit, ...params },
  });
  return response.data.data;
};

export const getAdminAuditLog = async (id: number): Promise<AdminAuditLog> => {
  const response = await apiClient.get(`/admin-audit-logs/${id}`);
  return response.data.data;
};
