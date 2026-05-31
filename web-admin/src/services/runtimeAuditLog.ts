import apiClient from './api';

export interface RuntimeAuditLog {
  id: number;
  request_id: string | null;
  session_id: number | null;
  agent_id: number | null;
  plugin_id: number | null;
  function_id: number | null;
  capability: string | null;
  event_type: string;
  outcome: string;
  elapsed_ms: number | null;
  error_message: string | null;
  payload_summary: Record<string, unknown> | null;
  occurred_at: string;
}

export interface RuntimeAuditLogListResponse {
  items: RuntimeAuditLog[];
  total: number;
  offset: number;
  limit: number;
}

export interface RuntimeAuditLogSearchParams {
  event_type?: string;
  outcome?: string;
  capability?: string;
  request_id?: string;
  session_id?: number;
  agent_id?: number;
  occurred_at_start?: string;
  occurred_at_end?: string;
}

export const getRuntimeAuditLogs = async (
  offset: number = 0,
  limit: number = 20,
  params?: RuntimeAuditLogSearchParams
): Promise<RuntimeAuditLogListResponse> => {
  const response = await apiClient.get('/runtime-audit-logs', {
    params: { offset, limit, ...params },
  });
  return response.data;
};

export const getRuntimeAuditLog = async (id: number): Promise<RuntimeAuditLog> => {
  const response = await apiClient.get(`/runtime-audit-logs/${id}`);
  return response.data;
};
