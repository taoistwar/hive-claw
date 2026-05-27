// 004 US2 — Tool CRUD (T089)
import apiClient from './api';

export interface ToolItem {
  id: number;
  identifier: string;
  name: string;
  description: string;
  kind: number;
  function_id: number | null;
  workflow_id: number | null;
  input_schema: unknown;
  output_schema: unknown;
  created_at: string;
  updated_at: string;
}

export interface ToolList {
  items: ToolItem[];
  total: number;
  offset: number;
  limit: number;
}

export interface ToolSearchParams {
  offset?: number;
  limit?: number;
  search?: string;
  kind?: number;
  created_at_start?: string;
  created_at_end?: string;
  updated_at_start?: string;
  updated_at_end?: string;
}

export async function listTools(params: ToolSearchParams = {}): Promise<ToolList> {
  const q: Record<string, string> = {};
  if (params.offset !== undefined) q.offset = String(params.offset);
  if (params.limit !== undefined) q.limit = String(params.limit);
  if (params.search) q.search = params.search;
  if (params.kind !== undefined) q.kind = String(params.kind);
  if (params.created_at_start) q.created_at_start = params.created_at_start;
  if (params.created_at_end) q.created_at_end = params.created_at_end;
  if (params.updated_at_start) q.updated_at_start = params.updated_at_start;
  if (params.updated_at_end) q.updated_at_end = params.updated_at_end;
  const resp = await apiClient.get<ToolList>('/tools', { params: q });
  return resp.data;
}

export interface CreateTool {
  identifier: string;
  name: string;
  description: string;
  kind: 1 | 2;
  function_id?: number;
  workflow_id?: number;
  input_schema: unknown;
  output_schema: unknown;
}

export async function createTool(meta: CreateTool): Promise<ToolItem> {
  const resp = await apiClient.post<ToolItem>('/tools', meta);
  return resp.data;
}

export interface UpdateTool {
  name?: string;
  description?: string;
  input_schema?: unknown;
  output_schema?: unknown;
  updated_at: string;
}

export async function updateTool(id: number, meta: UpdateTool): Promise<ToolItem> {
  const resp = await apiClient.put<ToolItem>(`/tools/${id}`, meta);
  return resp.data;
}

export async function deleteTool(id: number): Promise<void> {
  await apiClient.delete(`/tools/${id}`);
}
