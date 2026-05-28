// 004 US3 — Workflow CRUD + graph + execute (T112)
import apiClient from './api';

export interface WorkflowMeta {
  id: number;
  identifier: string;
  name: string;
  description: string | null;
  timeout_ms: number;
  category_id: number | null;
  required_capabilities: string[] | null;
  tags?: { id: number; name: string }[];
  created_at: string;
  updated_at: string;
}

export interface WorkflowList {
  items: WorkflowMeta[];
  total: number;
}

export interface GraphNode {
  id?: number;
  node_key: string;
  function_id: number;
  position?: { x: number; y: number } | null;
}

export interface GraphEdge {
  id?: number;
  src_node_key: string;
  dst_node_key: string;
  mapping: Record<string, string>;
}

export interface WorkflowGraph {
  workflow: WorkflowMeta;
  nodes: GraphNode[];
  edges: GraphEdge[];
}

export async function listWorkflows(params?: {
  offset?: number;
  limit?: number;
  search?: string;
  category_id?: number;
  tag_id?: number;
  created_at_from?: string;
  created_at_to?: string;
  updated_at_from?: string;
  updated_at_to?: string;
}): Promise<WorkflowList> {
  const q: Record<string, string> = {};
  if (params?.offset !== undefined) q.offset = String(params.offset);
  if (params?.limit !== undefined) q.limit = String(params.limit);
  if (params?.search) q.search = params.search;
  if (params?.category_id !== undefined) q.category_id = String(params.category_id);
  if (params?.tag_id !== undefined) q.tag_id = String(params.tag_id);
  if (params?.created_at_from) q.created_at_from = params.created_at_from;
  if (params?.created_at_to) q.created_at_to = params.created_at_to;
  if (params?.updated_at_from) q.updated_at_from = params.updated_at_from;
  if (params?.updated_at_to) q.updated_at_to = params.updated_at_to;
  const resp = await apiClient.get<WorkflowList>('/workflows', { params: q });
  return resp.data;
}

export async function createWorkflow(meta: {
  identifier: string;
  name: string;
  description?: string;
  timeout_ms?: number;
  category_id?: number;
  tag_ids?: number[];
}): Promise<WorkflowMeta> {
  const resp = await apiClient.post<WorkflowMeta>('/workflows', meta);
  return resp.data;
}

export async function getWorkflowGraph(id: number): Promise<WorkflowGraph> {
  const resp = await apiClient.get<WorkflowGraph>(`/workflows/${id}/graph`);
  return resp.data;
}

export async function putWorkflowGraph(
  id: number,
  graph: { nodes: GraphNode[]; edges: GraphEdge[] },
): Promise<WorkflowGraph> {
  const resp = await apiClient.put<WorkflowGraph>(`/workflows/${id}/graph`, graph);
  return resp.data;
}

export async function deleteWorkflow(id: number): Promise<void> {
  await apiClient.delete(`/workflows/${id}`);
}

export async function updateWorkflow(
  id: number,
  meta: {
    name?: string;
    description?: string;
    timeout_ms?: number;
    category_id?: number;
    tag_ids?: number[];
    updated_at: string;
  },
): Promise<WorkflowMeta> {
  const resp = await apiClient.put<WorkflowMeta>(`/workflows/${id}`, meta);
  return resp.data;
}
