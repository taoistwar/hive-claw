// 004 US5 — Agent CRUD + tree + model presets (T121)
import apiClient from './api';

export interface AgentItem {
  id: number;
  identifier: string;
  name: string;
  description: string | null;
  system_prompt: string;
  parent_agent_id: number | null;
  depth: number;
  model_preset: string | null;
  created_at: string;
  updated_at: string;
}

export interface AgentTreeNode extends AgentItem {
  children: AgentTreeNode[];
}

export interface AgentBriefRef {
  id: number;
  identifier: string;
  name: string;
}

export interface AgentDetail extends AgentItem {
  tools: AgentBriefRef[];
  skills: AgentBriefRef[];
  permissions: string[];
}

export interface ModelPreset {
  name: string;
  description: string;
  is_default: boolean;
}

export async function listAgentTree(): Promise<AgentTreeNode[]> {
  const resp = await apiClient.get<AgentTreeNode[]>('/agents');
  return resp.data;
}

export async function getAgent(id: number): Promise<AgentDetail> {
  const resp = await apiClient.get<AgentDetail>(`/agents/${id}`);
  return resp.data;
}

export async function listModelPresets(): Promise<ModelPreset[]> {
  const resp = await apiClient.get<ModelPreset[]>('/agents/model-presets');
  return resp.data;
}

export interface CreateAgent {
  identifier: string;
  name: string;
  description?: string;
  system_prompt: string;
  parent_agent_id?: number;
  model_preset?: string;
  tool_ids?: number[];
  skill_ids?: number[];
  permissions?: string[];
}

export async function createAgent(meta: CreateAgent): Promise<AgentDetail> {
  const resp = await apiClient.post<AgentDetail>('/agents', meta);
  return resp.data;
}

export interface UpdateAgent {
  name?: string;
  description?: string;
  system_prompt?: string;
  model_preset?: string;
  tool_ids?: number[];
  skill_ids?: number[];
  permissions?: string[];
  updated_at: string;
}

export async function updateAgent(id: number, meta: UpdateAgent): Promise<AgentDetail> {
  const resp = await apiClient.put<AgentDetail>(`/agents/${id}`, meta);
  return resp.data;
}

export async function deleteAgent(id: number): Promise<void> {
  await apiClient.delete(`/agents/${id}`);
}
