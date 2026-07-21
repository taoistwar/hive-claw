# API 文档

## 对外 API

以下 API 无需 JWT 鉴权，对所有外部调用方开放。

| 接口 | 方法 | 说明 |
|------|------|------|
| [AI 助手对话](assistant.md) | `POST /api/assistant` | 向 AI 助手发送消息并获取回复 |
| [获取用户历史消息](messages.md) | `POST /api/messages` | 获取指定时间之前的最近 N 条聊天记录 |
| [创建新会话](newsession.md) | `POST /api/newsession` | 为用户创建新的聊天会话 |

## 已废弃 API（Legacy）

> **推荐游戏业务已废弃。** 对应的管理端页面、后台 CRUD 和以下公开 API 仅为兼容
> 历史调用保留，不再作为可用业务能力。新客户端和新代码不得调用、依赖或扩展这些
> 接口；链接中的文档仅用于排查遗留流量与数据。

| 历史接口 | 方法 | 状态 |
|---------|------|------|
| [获取热门推荐游戏](recommended-games-top.md) | `POST /api/recommended-games/top` | Legacy，仅兼容保留 |
| [执行推荐游戏](recommended-games-execute.md) | `POST /api/recommended-games/execute` | Legacy，仅兼容保留 |

## 内部 API

详见 [内部 API](internal.md)。
