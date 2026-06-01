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
  const resp = await apiClient.get<SessionList>('/chat/sessions', { params });
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
    } catch {}
    onEvent({
      type: 'error',
      code: resp.status,
      message: bodyText || resp.statusText || 'Network error',
    });
    return;
  }

  const reader = resp.body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';

  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;

      buffer += decoder.decode(value, { stream: true });
      const lines = buffer.split('\n');
      buffer = lines.pop() || '';

      for (const line of lines) {
        if (!line.startsWith('data: ')) continue;
        const jsonStr = line.slice(6);
        try {
          const event: SseEvent = JSON.parse(jsonStr);
          onEvent(event);
        } catch {}
      }
    }
  } catch (err: any) {
    if (err.name !== 'AbortError') {
      onEvent({ type: 'error', code: 0, message: err.message || 'Stream interrupted' });
    }
  }
}
