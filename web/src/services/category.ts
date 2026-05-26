// 004 US7 — Category CRUD (T135)
import apiClient from './api';

export interface CategoryItem {
  id: number;
  parent_id: number | null;
  name: string;
  slug: string;
  description: string | null;
  created_at: string;
  updated_at: string;
}

export interface CategoryNode extends CategoryItem {
  children: CategoryNode[];
}

export async function listCategoriesTree(): Promise<CategoryNode[]> {
  const resp = await apiClient.get<CategoryNode[]>('/categories');
  return resp.data;
}

export async function listCategoriesFlat(): Promise<CategoryItem[]> {
  const resp = await apiClient.get<CategoryItem[]>('/categories', { params: { flat: 1 } });
  return resp.data;
}

export async function createCategory(meta: {
  parent_id?: number;
  name: string;
  slug: string;
  description?: string;
}): Promise<CategoryItem> {
  const resp = await apiClient.post<CategoryItem>('/categories', meta);
  return resp.data;
}

export async function updateCategory(
  id: number,
  meta: {
    name?: string;
    slug?: string;
    description?: string;
    parent_id?: number | null;
    updated_at: string;
  },
): Promise<CategoryItem> {
  const resp = await apiClient.put<CategoryItem>(`/categories/${id}`, meta);
  return resp.data;
}

export async function deleteCategory(id: number): Promise<void> {
  await apiClient.delete(`/categories/${id}`);
}
