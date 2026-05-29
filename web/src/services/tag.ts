// 004 US7 — Tag CRUD (T135)
import apiClient from './api';

export interface TagItem {
  id: number;
  name: string;
  color: string | null;
  created_at: string;
  reference_count: number;
}

export interface TagListResponse {
  items: TagItem[];
  total: number;
  offset: number;
  limit: number;
}

export async function listTags(q?: string, offset?: number, limit?: number): Promise<TagListResponse> {
  const params: Record<string, string> = {};
  if (q) params.q = q;
  if (offset !== undefined) params.offset = String(offset);
  if (limit !== undefined) params.limit = String(limit);
  const resp = await apiClient.get<TagListResponse>('/tags', { params });
  return resp.data;
}

export async function createTag(meta: { name: string; color?: string }): Promise<TagItem> {
  const resp = await apiClient.post<TagItem>('/tags', meta);
  return resp.data;
}

export async function updateTag(
  id: number,
  meta: { name?: string; color?: string },
): Promise<TagItem> {
  const resp = await apiClient.put<TagItem>(`/tags/${id}`, meta);
  return resp.data;
}

export async function deleteTag(id: number): Promise<void> {
  await apiClient.delete(`/tags/${id}`);
}
