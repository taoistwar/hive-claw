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
}): Promise<SkillList> {
  const q: Record<string, string> = {};
  if (params.offset !== undefined) q.offset = String(params.offset);
  if (params.limit !== undefined) q.limit = String(params.limit);
  if (params.search) q.search = params.search;
  if (params.source) q.source = params.source;
  const resp = await apiClient.get<SkillList>('/skills', { params: q });
  return resp.data;
}

export interface CreateSkill {
  identifier: string;
  name: string;
  description: string;
  frontmatter?: unknown;
  content: string;
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
  updated_at: string;
}

export async function updateSkill(id: number, meta: UpdateSkill): Promise<SkillItem> {
  const resp = await apiClient.put<SkillItem>(`/skills/${id}`, meta);
  return resp.data;
}

export async function deleteSkill(id: number): Promise<void> {
  await apiClient.delete(`/skills/${id}`);
}
