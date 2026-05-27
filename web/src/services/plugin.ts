// 004 Agent Runtime — Plugin CRUD + multipart upload + 三维检索 (T074)

import apiClient from './api';

export interface PluginTag {
  id: number;
  name: string;
}

export interface Plugin {
  id: number;
  identifier: string;
  name: string;
  description: string | null;
  manifest: unknown | null;
  runtime: string;
  version: string;
  author: string | null;
  repository_url: string | null;
  s3_key: string;
  sha256: string;
  size_bytes: number;
  category_id: number | null;
  created_at: string;
  updated_at: string;
  deleted_at: string | null;
  tags?: PluginTag[];
}

export interface PluginList {
  items: Plugin[];
  total: number;
  offset: number;
  limit: number;
}

export interface PluginListParams {
  offset?: number;
  limit?: number;
  search?: string;
  category_id?: number;
  tag_ids?: number[];
  include_deleted?: boolean;
}

export async function listPlugins(params: PluginListParams = {}): Promise<PluginList> {
  const query: Record<string, string> = {};
  if (params.offset !== undefined) query.offset = String(params.offset);
  if (params.limit !== undefined) query.limit = String(params.limit);
  if (params.search) query.search = params.search;
  if (params.category_id !== undefined) query.category_id = String(params.category_id);
  if (params.tag_ids?.length) query.tag_ids = params.tag_ids.join(',');
  if (params.include_deleted) query.include_deleted = 'true';
  const resp = await apiClient.get<PluginList>('/plugins', { params: query });
  // apiClient response interceptor 已 unwrap envelope.data
  return resp.data;
}

export async function getPlugin(id: number): Promise<Plugin> {
  const resp = await apiClient.get<Plugin>(`/plugins/${id}`);
  return resp.data;
}

export interface UploadMeta {
  identifier: string;
  name: string;
  version: string;
  description?: string;
  runtime?: string; // default "extism"
  author?: string;
  repository_url?: string;
  category_id?: number;
  tag_ids?: number[];
}

export async function uploadPlugin(file: File, meta: UploadMeta): Promise<Plugin> {
  const form = new FormData();
  form.append('file', file);
  form.append('meta', JSON.stringify(meta));
  const resp = await apiClient.post<Plugin>('/plugins', form, {
    headers: { 'Content-Type': 'multipart/form-data' },
  });
  return resp.data;
}

export interface UpdateMeta {
  name?: string;
  description?: string;
  author?: string;
  repository_url?: string;
  category_id?: number;
  tag_ids?: number[];
  /** 乐观锁：必须传 GET 时拿到的 updated_at */
  updated_at: string;
}

export async function updatePlugin(id: number, meta: UpdateMeta): Promise<Plugin> {
  const resp = await apiClient.put<Plugin>(`/plugins/${id}`, meta);
  return resp.data;
}

export async function deletePlugin(id: number): Promise<void> {
  await apiClient.delete(`/plugins/${id}`);
}

/** 下载 WASM 文件（绕过 axios JSON envelope 拦截器） */
export async function downloadPlugin(plugin: Plugin): Promise<void> {
  const token = localStorage.getItem('auth_token');
  const API_BASE_URL = import.meta.env.VITE_API_BASE_URL || import.meta.env.VITE_API_URL || 'http://localhost:3000/api';
  const url = `${API_BASE_URL}/plugins/${plugin.id}/download`;
  const resp = await fetch(url, {
    headers: { Authorization: `Bearer ${token}` },
  });
  if (!resp.ok) {
    throw new Error(`下载失败：HTTP ${resp.status}`);
  }
  const blob = await resp.blob();
  const filename = `${plugin.identifier}-${plugin.version}.wasm`;
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob);
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(a.href);
}
