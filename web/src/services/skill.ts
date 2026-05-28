// 004 US2 — Skill CRUD (T089)
import apiClient from './api';

export interface SkillItem {
  id: number;
  identifier: string;
  name: string;
  description: string;
  frontmatter: unknown | null;
  content: string;
  source: 'workspace' | 'builtin';
  is_always: boolean;
  category_id: number | null;
  required_capabilities: string[] | null;
  created_at: string;
  updated_at: string;
}

export interface SkillList {
  items: SkillItem[];
  total: number;
  offset: number;
  limit: number;
}

export async function listSkills(params: {
  offset?: number;
  limit?: number;
  search?: string;
  source?: 'workspace' | 'builtin';
  category_id?: number;
  identifier?: string;
  name?: string;
  description?: string;
  required_capabilities?: string;
  created_at_start?: string;
  created_at_end?: string;
  updated_at_start?: string;
  updated_at_end?: string;
}): Promise<SkillList> {
  const q: Record<string, string> = {};
  if (params.offset !== undefined) q.offset = String(params.offset);
  if (params.limit !== undefined) q.limit = String(params.limit);
  if (params.search) q.search = params.search;
  if (params.source) q.source = params.source;
  if (params.category_id !== undefined) q.category_id = String(params.category_id);
  if (params.identifier) q.identifier = params.identifier;
  if (params.name) q.name = params.name;
  if (params.description) q.description = params.description;
  if (params.required_capabilities) q.required_capabilities = params.required_capabilities;
  if (params.created_at_start) q.created_at_start = params.created_at_start;
  if (params.created_at_end) q.created_at_end = params.created_at_end;
  if (params.updated_at_start) q.updated_at_start = params.updated_at_start;
  if (params.updated_at_end) q.updated_at_end = params.updated_at_end;
  const resp = await apiClient.get<SkillList>('/skills', { params: q });
  return resp.data;
}

export interface CreateSkill {
  identifier: string;
  name: string;
  description: string;
  frontmatter?: unknown;
  content: string;
  is_always?: boolean;
}

export async function createSkill(meta: CreateSkill): Promise<SkillItem> {
  const resp = await apiClient.post<SkillItem>('/skills', meta);
  return resp.data;
}

export interface UpdateSkill {
  name?: string;
  description?: string;
  frontmatter?: unknown;
  content?: string;
  is_always?: boolean;
  updated_at: string;
}

export async function updateSkill(id: number, meta: UpdateSkill): Promise<SkillItem> {
  const resp = await apiClient.put<SkillItem>(`/skills/${id}`, meta);
  return resp.data;
}

export async function deleteSkill(id: number): Promise<void> {
  await apiClient.delete(`/skills/${id}`);
}

export interface SkillTestRequest {
  message: string;
  model_preset?: string;
}

export interface SkillTestResult {
  assistant_content: string;
  has_tool_calls: boolean;
  tool_calls: SkillTestCall[];
}

export interface SkillTestCall {
  tool_name: string;
  arguments: unknown;
  result: SkillTestCallResult;
}

export interface SkillTestCallResult {
  success: boolean;
  content: unknown;
  error: string | null;
}

export async function testSkill(id: number, req: SkillTestRequest): Promise<SkillTestResult> {
  const resp = await apiClient.post<SkillTestResult>(`/skills/${id}/test`, req);
  return resp.data;
}
