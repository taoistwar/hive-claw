import apiClient from './api';

export interface ChatSession {
  id: number;
  user_id: number;
  title: string | null;
  created_at: string;
  updated_at: string;
}

export interface ChatMessage {
  id: number;
  session_id: number;
  seq: number;
  role: 'user' | 'assistant' | 'system';
  content: string | null;
  created_at: string;
}

export interface SessionList {
  items: ChatSession[];
  total: number;
}

export interface SessionListParams {
  offset?: number;
  limit?: number;
  search?: string;
}

export async function listSessions(params?: SessionListParams): Promise<SessionList> {
  const resp = await apiClient.get<SessionList>('/user-chat/sessions', { params });
  return resp.data;
}

export async function createSession(title?: string): Promise<ChatSession> {
  const resp = await apiClient.post<ChatSession>('/user-chat/sessions', { title });
  return resp.data;
}

export async function getMessages(sessionId: number): Promise<ChatMessage[]> {
  const resp = await apiClient.get<ChatMessage[]>(`/user-chat/sessions/${sessionId}/messages`);
  return resp.data;
}

export async function deleteSession(sessionId: number): Promise<void> {
  await apiClient.delete(`/user-chat/sessions/${sessionId}`);
}

export type SseEvent =
  | { type: 'token'; text: string }
  | { type: 'done'; elapsed_ms: number }
  | { type: 'error'; code: number; message: string };

export async function sendMessageStream(
  sessionId: number,
  content: string,
  onEvent: (e: SseEvent) => void,
  signal?: AbortSignal,
): Promise<void> {
  const baseUrl =
    import.meta.env.VITE_API_BASE_URL ||
    import.meta.env.VITE_API_URL ||
    '/api';
  const token = localStorage.getItem('user_auth_token') ?? '';
  const resp = await fetch(`${baseUrl}/user-chat/sessions/${sessionId}/messages`, {
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
    } catch {}
    onEvent({
      type: 'error',
      code: resp.status,
      message: bodyText || resp.statusText || 'Network error',
    });
    return;
  }

  const reader = resp.body.getReader();
  const decoder = new TextDecoder('utf-8');
  let buf = '';
  let currentEvent = '';

  try {
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
        if (line.startsWith(':')) continue;
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
  } catch (err: any) {
    if (err.name !== 'AbortError') {
      onEvent({ type: 'error', code: 0, message: err.message || 'Stream interrupted' });
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
    case 'done':
      emit({ type: 'done', elapsed_ms: Number(payload.elapsed_ms ?? 0) });
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
