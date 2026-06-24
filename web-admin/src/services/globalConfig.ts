import apiClient from './api';

export interface GlobalConfigItem {
  id: number;
  name: string;
  key: string;
  config_type: string;
  data: Record<string, unknown>;
  created_at: string;
  updated_at: string;
}

export interface GlobalConfigListResponse {
  items: GlobalConfigItem[];
  total: number;
  offset: number;
  limit: number;
}

export async function listGlobalConfigs(
  q?: string,
  offset?: number,
  limit?: number,
): Promise<GlobalConfigListResponse> {
  const params: Record<string, string> = {};
  if (q) params.q = q;
  if (offset !== undefined) params.offset = String(offset);
  if (limit !== undefined) params.limit = String(limit);
  const resp = await apiClient.get<GlobalConfigListResponse>('/global-configs', { params });
  return resp.data;
}

export async function createGlobalConfig(meta: {
  name: string;
  key: string;
  config_type: string;
  data: Record<string, unknown>;
}): Promise<GlobalConfigItem> {
  const resp = await apiClient.post<GlobalConfigItem>('/global-configs', meta);
  return resp.data;
}

export async function updateGlobalConfig(
  id: number,
  meta: {
    name?: string;
    config_type?: string;
    data?: Record<string, unknown>;
  },
): Promise<GlobalConfigItem> {
  const resp = await apiClient.put<GlobalConfigItem>(`/global-configs/${id}`, meta);
  return resp.data;
}

export async function deleteGlobalConfig(id: number): Promise<void> {
  await apiClient.delete(`/global-configs/${id}`);
}
