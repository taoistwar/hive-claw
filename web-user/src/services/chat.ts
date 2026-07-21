// 聊天服务 — 使用对外 API (MD5 签名)
import { sendMessage, getMessages, fetchTopRecommendedGames, executeRecommendation, createNewSession } from './api-external';
import type { ChatMessage, TopRecommendedGame, Extension, GameInfo, GameCardPayload, SubscribeInfo, DurationCard, SupportCardPayload } from './api-external';

export type { ChatMessage, TopRecommendedGame, Extension, GameInfo, GameCardPayload, SubscribeInfo, DurationCard, SupportCardPayload };
export { sendMessage, getMessages, fetchTopRecommendedGames, executeRecommendation, createNewSession };
