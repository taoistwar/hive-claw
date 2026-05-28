// 004 US2 — Tool CRUD (T089)
import apiClient from './api';

export interface TagItem {
  id: number;
  name: string;
  color?: string;
  reference_count?: number;
}

export async function listTags(query?: string): Promise<TagItem[]> {
  const params = query ? { q: query } : {};
  const resp = await apiClient.get<{ tag: TagItem; reference_count: number }[]>('/tags', { params });
  return resp.data.map((item) => item.tag);
}

export interface ToolItem {
  id: number;
  identifier: string;
  name: string;
  description: string;
  kind: number;
  source: string;
  is_always: boolean;
  function_id: number | null;
  workflow_id: number | null;
  input_schema: unknown;
  output_schema: unknown;
  category_id: number | null;
  required_capabilities: string[] | null;
  created_at: string;
  updated_at: string;
  tags: TagItem[];
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
  source?: string;
  category_id?: number;
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
  if (params.source) q.source = params.source;
  if (params.category_id !== undefined) q.category_id = String(params.category_id);
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
  source?: 'workspace' | 'builtin';
  is_always?: boolean;
  function_id?: number;
  workflow_id?: number;
  input_schema: unknown;
  output_schema: unknown;
  category_id?: number;
  required_capabilities?: string[];
  tag_ids?: number[];
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
  category_id?: number | null;
  required_capabilities?: string[];
  tag_ids?: number[];
  is_always?: boolean;
  updated_at: string;
}

export async function updateTool(id: number, meta: UpdateTool): Promise<ToolItem> {
  const resp = await apiClient.put<ToolItem>(`/tools/${id}`, meta);
  return resp.data;
}

export async function deleteTool(id: number): Promise<void> {
  await apiClient.delete(`/tools/${id}`);
}

export interface ToolTestRequest {
  message: string;
  model_preset?: string;
}

export interface ToolTestResult {
  assistant_content: string;
  has_tool_calls: boolean;
  tool_calls: ToolTestCall[];
}

export interface ToolTestCall {
  tool_name: string;
  arguments: unknown;
  result: ToolTestCallResult;
}

export interface ToolTestCallResult {
  success: boolean;
  content: unknown;
  error: string | null;
}

export async function testTool(id: number, req: ToolTestRequest): Promise<ToolTestResult> {
  const resp = await apiClient.post<ToolTestResult>(`/tools/${id}/test`, req);
  return resp.data;
}
