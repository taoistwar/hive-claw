// 004 US6 — Chat session CRUD + SSE event streaming (T129)

import apiClient from './api';

export interface ChatSession {
  id: number;
  admin_id: number | null;
  admin_phone_snapshot: string;
  admin_nickname_snapshot: string;
  title: string | null;
  created_at: string;
  updated_at: string;
}

export interface ChatMessage {
  id: number;
  session_id: number;
  seq: number;
  role: 'user' | 'assistant' | 'tool' | 'system';
  content: string | null;
  tool_calls: unknown | null;
  routed_to_agent_id: number | null;
  elapsed_ms: number | null;
  created_at: string;
}

export interface SessionList {
  items: ChatSession[];
  total: number;
}

export async function listSessions(): Promise<SessionList> {
  const resp = await apiClient.get<SessionList>('/chat/sessions');
  return resp.data;
}

export async function createSession(title?: string): Promise<ChatSession> {
  const resp = await apiClient.post<ChatSession>('/chat/sessions', { title });
  return resp.data;
}

export async function getMessages(sessionId: number): Promise<ChatMessage[]> {
  const resp = await apiClient.get<ChatMessage[]>(`/chat/sessions/${sessionId}/messages`);
  return resp.data;
}

export async function deleteSession(sessionId: number): Promise<void> {
  await apiClient.delete(`/chat/sessions/${sessionId}`);
}

// =================== SSE 流式发送 ===================

export type SseEvent =
  | { type: 'token'; text: string }
  | { type: 'tool_call'; tool_call_id: string; name: string; args: unknown }
  | { type: 'tool_result'; tool_call_id: string; result: unknown }
  | { type: 'routed'; agent_id: number; agent_identifier: string }
  | { type: 'fallback_used'; from: string; to: string; reason: string }
  | { type: 'done'; elapsed_ms: number; final_agent_id: number | null }
  | { type: 'error'; code: number; message: string };

/**
 * 发送 chat 消息 → 解析 SSE 流，回调每个事件。
 * 用 fetch + ReadableStream 手动解析（标准 EventSource 不能用 axios+POST+headers）。
 */
export async function sendMessageStream(
  sessionId: number,
  content: string,
  onEvent: (e: SseEvent) => void,
  signal?: AbortSignal,
): Promise<void> {
  const baseUrl =
    import.meta.env.VITE_API_BASE_URL ||
    import.meta.env.VITE_API_URL ||
    'http://localhost:3000/api';
  const token = localStorage.getItem('auth_token') ?? '';
  const resp = await fetch(`${baseUrl}/chat/sessions/${sessionId}/messages`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Accept: 'text/event-stream',
      Authorization: `Bearer ${token}`,
    },
    body: JSON.stringify({ content }),
    signal,
  });

  if (!resp.ok || !resp.body) {
    let bodyText = '';
    try {
      bodyText = await resp.text();
    } catch {
      // ignore
    }
    onEvent({
      type: 'error',
      code: resp.status,
      message: bodyText || `HTTP ${resp.status}`,
    });
    return;
  }

  const reader = resp.body.getReader();
  const decoder = new TextDecoder('utf-8');
  let buf = '';
  let currentEvent = '';

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    buf += decoder.decode(value, { stream: true });
    let idx;
    while ((idx = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, idx).trim();
      buf = buf.slice(idx + 1);
      if (!line) {
        currentEvent = '';
        continue;
      }
      if (line.startsWith(':')) continue; // SSE comment (keepalive)
      if (line.startsWith('event:')) {
        currentEvent = line.slice(6).trim();
        continue;
      }
      if (line.startsWith('data:')) {
        const data = line.slice(5).trim();
        try {
          const payload = JSON.parse(data);
          dispatchEvent(currentEvent, payload, onEvent);
        } catch {
          // skip malformed
        }
      }
    }
  }
}

function dispatchEvent(
  type: string,
  payload: Record<string, unknown>,
  emit: (e: SseEvent) => void,
) {
  switch (type) {
    case 'token':
      emit({ type: 'token', text: String(payload.text ?? '') });
      break;
    case 'tool_call':
      emit({
        type: 'tool_call',
        tool_call_id: String(payload.tool_call_id),
        name: String(payload.name),
        args: payload.args,
      });
      break;
    case 'tool_result':
      emit({
        type: 'tool_result',
        tool_call_id: String(payload.tool_call_id),
        result: payload.result,
      });
      break;
    case 'routed':
      emit({
        type: 'routed',
        agent_id: Number(payload.agent_id),
        agent_identifier: String(payload.agent_identifier ?? ''),
      });
      break;
    case 'fallback_used':
      emit({
        type: 'fallback_used',
        from: String(payload.from ?? ''),
        to: String(payload.to ?? ''),
        reason: String(payload.reason ?? ''),
      });
      break;
    case 'done':
      emit({
        type: 'done',
        elapsed_ms: Number(payload.elapsed_ms ?? 0),
        final_agent_id:
          payload.final_agent_id !== null && payload.final_agent_id !== undefined
            ? Number(payload.final_agent_id)
            : null,
      });
      break;
    case 'error':
      emit({
        type: 'error',
        code: Number(payload.code ?? 0),
        message: String(payload.message ?? ''),
      });
      break;
  }
}
