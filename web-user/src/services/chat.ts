// 聊天服务 — 使用对外 API (MD5 签名)
import { sendMessage, getMessages } from './api-external';
import type { ChatMessage, AssistantResponse } from './api-external';

export type { ChatMessage, AssistantResponse };
export { sendMessage, getMessages };
