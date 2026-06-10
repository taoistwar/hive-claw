import md5 from 'blueimp-md5';

// 对外 API 服务层 — MD5 签名鉴权
const API_BASE_URL = import.meta.env.VITE_API_BASE_URL || import.meta.env.VITE_API_URL || '/api';

function getSecret(): string {
  return import.meta.env.VITE_ASSISTANT_SECRET || '';
}

/** 计算 MD5 签名 */
function computeSign(secret: string, signString: string): string {
  return md5(secret + signString);
}

// ── 对外 API 类型 ──

/** 单条聊天消息（与后端 chat_messages_user 表对应） */
export interface ChatMessage {
  id: number;
  session_id: number;
  user_id: number;
  role: string;
  content: string | null;
  elapsed_ms: number | null;
  /** 扩展数据数组：cards、images、suggestions 等（flattened） */
  extensions?: unknown[] | null;
  created_at: string;
}

export interface MessagesResponse {
  messages: ChatMessage[];
}

/** 热门推荐游戏 */
export interface TopRecommendedGame {
  name: string;
  reply: string;
  reason: string | null;
  tag: string | null;
  game_category: unknown | null;
  game_image: string | null;
  game_id: string;
  game_name: string;
}

export interface TopRecommendedGamesResponse {
  code: number;
  message: string;
  data: TopRecommendedGame[];
}

// ── 对外 API 函数 ──

/** 发送消息到 AI 助手，返回完整持久化的 ChatMessage 记录 (POST /api/assistant?sign={md5}) */
export async function sendMessage(params: {
  user_id: number;
  message: string;
  channel: string;
  client_type: string;
  client_version: string;
  new_session?: boolean;
}): Promise<ChatMessage> {
  const body = JSON.stringify(params);
  const secret = getSecret();

  let url = `${API_BASE_URL}/assistant`;
  if (secret) {
    const signStr = `/api/assistant?body=${body}`;
    const sign = computeSign(secret, signStr);
    url += `?sign=${encodeURIComponent(sign)}`;
  }

  const resp = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json; charset=UTF-8' },
    body,
  });

  if (!resp.ok) {
    const text = await resp.text().catch(() => '');
    throw new Error(text || `HTTP ${resp.status}`);
  }

  return resp.json();
}

/** 获取用户历史消息 (POST /api/messages?sign={md5}) */
export async function getMessages(params: {
  user_id: number;
  date: string; // YYYY-MM-DD HH:MM:SS
}): Promise<ChatMessage[]> {
  const body = JSON.stringify({ user_id: params.user_id, date: params.date });
  const secret = getSecret();

  let url = `${API_BASE_URL}/messages`;
  if (secret) {
    const signStr = `/api/messages?body=${body}`;
    const sign = computeSign(secret, signStr);
    url += `?sign=${encodeURIComponent(sign)}`;
  }

  const resp = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json; charset=UTF-8' },
    body,
  });

  if (!resp.ok) {
    const text = await resp.text().catch(() => '');
    throw new Error(text || `HTTP ${resp.status}`);
  }

  const data: MessagesResponse = await resp.json();
  return data.messages;
}

/** 获取热门推荐游戏 (POST /api/recommended-games/top?sign={md5}) */
export async function fetchTopRecommendedGames(params: {
  user_id: number;
  channel: string;
  client_type: string;
  client_version: string;
}): Promise<TopRecommendedGame[]> {
  // Note: user_id must be serialized as a string — the backend TopRequest expects String
  const body = JSON.stringify({ ...params, user_id: String(params.user_id) });
  const secret = getSecret();

  let url = `${API_BASE_URL}/recommended-games/top`;
  if (secret) {
    const signStr = `/api/recommended-games/top?body=${body}`;
    const sign = computeSign(secret, signStr);
    url += `?sign=${encodeURIComponent(sign)}`;
  }

  const resp = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json; charset=UTF-8' },
    body,
  });

  if (!resp.ok) {
    const text = await resp.text().catch(() => '');
    throw new Error(text || `HTTP ${resp.status}`);
  }

  const result: TopRecommendedGamesResponse = await resp.json();
  return result.data ?? [];
}

/** 执行推荐：记录推荐点击为两条聊天消息 (POST /api/recommended-games/execute?sign={md5}) */
export async function executeRecommendation(params: {
  user_id: number;
  game_id: string;
  channel: string;
  client_type: string;
  client_version: string;
}): Promise<ChatMessage> {
  const body = JSON.stringify({ ...params, user_id: String(params.user_id) });
  const secret = getSecret();

  let url = `${API_BASE_URL}/recommended-games/execute`;
  if (secret) {
    const signStr = `/api/recommended-games/execute?body=${body}`;
    const sign = computeSign(secret, signStr);
    url += `?sign=${encodeURIComponent(sign)}`;
  }

  const resp = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json; charset=UTF-8' },
    body,
  });

  if (!resp.ok) {
    const text = await resp.text().catch(() => '');
    throw new Error(text || `HTTP ${resp.status}`);
  }

  const result: { code: number; data: ChatMessage } = await resp.json();
  return result.data;
}
