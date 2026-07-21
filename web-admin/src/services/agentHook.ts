import { apiClient } from './api';

// ---- Interfaces ----

export interface AgentHook {
  id: number;
  agent_id: number;
  name: string;
  description: string | null;
  trigger_point: TriggerPoint;
  action_type: ActionType;
  action_params: Record<string, unknown>;
  enabled: boolean;
  sort_order: number;
  blocking_mode: boolean;
  timeout_ms: number;
  created_at: string;
  updated_at: string;
}

export type TriggerPoint =
  | 'before_agent_start'
  | 'after_agent_end'
  | 'on_agent_error'
  | 'before_tool_call'
  | 'after_tool_call'
  | 'before_llm_call'
  | 'after_llm_call';

export type ActionType = 'call_function' | 'call_workflow' | 'http_webhook';

export interface CreateHookRequest {
  name: string;
  description?: string;
  trigger_point: TriggerPoint;
  action_type: ActionType;
  action_params: Record<string, unknown>;
  enabled?: boolean;
  sort_order?: number;
  blocking_mode?: boolean;
  timeout_ms?: number;
}

export interface UpdateHookRequest {
  name?: string;
  description?: string | null;
  trigger_point?: TriggerPoint;
  action_type?: ActionType;
  action_params?: Record<string, unknown>;
  enabled?: boolean;
  sort_order?: number;
  blocking_mode?: boolean;
  timeout_ms?: number;
  updated_at: string;
}

// ---- API Functions ----

export async function listHooks(agentId: number): Promise<AgentHook[]> {
  const resp = await apiClient.get<AgentHook[]>(`/agents/${agentId}/hooks`);
  return resp.data;
}

export async function createHook(agentId: number, meta: CreateHookRequest): Promise<AgentHook> {
  const resp = await apiClient.post<AgentHook>(`/agents/${agentId}/hooks`, meta);
  return resp.data;
}

export async function updateHook(
  agentId: number,
  hookId: number,
  meta: UpdateHookRequest,
): Promise<AgentHook> {
  const resp = await apiClient.put<AgentHook>(`/agents/${agentId}/hooks/${hookId}`, meta);
  return resp.data;
}

export async function deleteHook(agentId: number, hookId: number): Promise<void> {
  await apiClient.delete(`/agents/${agentId}/hooks/${hookId}`);
}

// ---- Constants ----

export const TRIGGER_POINT_LABELS: Record<TriggerPoint, string> = {
  before_agent_start: 'Agent 开始前',
  after_agent_end: 'Agent 结束后',
  on_agent_error: 'Agent 出错时',
  before_tool_call: 'Tool 调用前',
  after_tool_call: 'Tool 调用后',
  before_llm_call: 'LLM 调用前',
  after_llm_call: 'LLM 调用后',
};

export const ACTION_TYPE_LABELS: Record<ActionType, string> = {
  call_function: '调用 Function',
  call_workflow: '调用 Workflow',
  http_webhook: 'HTTP Webhook',
};
