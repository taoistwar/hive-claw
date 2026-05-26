// 004 US3 — Workflow CRUD + graph + execute (T112)
import apiClient from './api';

export interface WorkflowMeta {
  id: number;
  identifier: string;
  name: string;
  description: string | null;
  timeout_ms: number;
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

export async function listWorkflows(offset = 0, limit = 20): Promise<WorkflowList> {
  const resp = await apiClient.get<WorkflowList>('/workflows', { params: { offset, limit } });
  return resp.data;
}

export async function createWorkflow(meta: {
  identifier: string;
  name: string;
  description?: string;
  timeout_ms?: number;
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
