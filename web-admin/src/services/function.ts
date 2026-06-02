// 004 US2 — Function CRUD (T089)
import apiClient from './api';

export interface FunctionItem {
  id: number;
  identifier: string;
  name: string;
  description: string | null;
  kind: number; // 1 builtin, 2 custom
  input_schema: unknown;
  output_schema: unknown;
  plugin_id: number | null;
  plugin_export: string | null;
  category_id: number | null;
  required_capabilities: string[] | null;
  created_at: string;
  updated_at: string;
  plugin_identifier?: string | null;
  tags?: { id: number; name: string }[];
}

export interface FunctionList {
  items: FunctionItem[];
  total: number;
  offset: number;
  limit: number;
}

export async function listFunctions(params: {
  offset?: number;
  limit?: number;
  search?: string;
  category_id?: number;
  kind?: 'builtin' | 'custom';
  identifier?: string;
  name?: string;
  plugin_id?: number;
  plugin_identifier?: string;
  required_capabilities?: string;
  tag_id?: number;
  created_at_start?: string;
  created_at_end?: string;
  updated_at_start?: string;
  updated_at_end?: string;
}): Promise<FunctionList> {
  const q: Record<string, string> = {};
  if (params.offset !== undefined) q.offset = String(params.offset);
  if (params.limit !== undefined) q.limit = String(params.limit);
  if (params.search) q.search = params.search;
  if (params.category_id !== undefined) q.category_id = String(params.category_id);
  if (params.kind) q.kind = params.kind;
  if (params.identifier) q.identifier = params.identifier;
  if (params.name) q.name = params.name;
  if (params.plugin_id !== undefined) q.plugin_id = String(params.plugin_id);
  if (params.plugin_identifier) q.plugin_identifier = params.plugin_identifier;
  if (params.required_capabilities) q.required_capabilities = params.required_capabilities;
  if (params.tag_id !== undefined) q.tag_id = String(params.tag_id);
  if (params.created_at_start) q.created_at_start = params.created_at_start;
  if (params.created_at_end) q.created_at_end = params.created_at_end;
  if (params.updated_at_start) q.updated_at_start = params.updated_at_start;
  if (params.updated_at_end) q.updated_at_end = params.updated_at_end;
  const resp = await apiClient.get<FunctionList>('/functions', { params: q });
  return resp.data;
}

export interface CreateFunction {
  identifier: string;
  name: string;
  description?: string;
  plugin_id: number;
  plugin_export: string;
  input_schema: unknown;
  output_schema: unknown;
  category_id?: number;
  required_capabilities?: string[];
  tag_ids?: number[];
}

export async function createFunction(meta: CreateFunction): Promise<FunctionItem> {
  const resp = await apiClient.post<FunctionItem>('/functions', meta);
  return resp.data;
}

export interface UpdateFunction {
  name?: string;
  description?: string;
  category_id?: number;
  input_schema?: unknown;
  output_schema?: unknown;
  required_capabilities?: string[];
  tag_ids?: number[];
  updated_at: string;
}

export async function updateFunction(id: number, meta: UpdateFunction): Promise<FunctionItem> {
  const resp = await apiClient.put<FunctionItem>(`/functions/${id}`, meta);
  return resp.data;
}

export async function deleteFunction(id: number): Promise<void> {
  await apiClient.delete(`/functions/${id}`);
}

export interface InvokeFunctionRequest {
  input: unknown;
  agent_id?: number;
}

export interface InvokeFunctionResponse {
  output: unknown;
  elapsed_ms: number;
}

export async function invokeFunction(id: number, params: InvokeFunctionRequest): Promise<InvokeFunctionResponse> {
  const resp = await apiClient.post<InvokeFunctionResponse>(`/functions/${id}/invoke`, params);
  return resp.data;
}
