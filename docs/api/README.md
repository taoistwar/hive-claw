# API 文档

## 对外 API

以下 API 无需 JWT 鉴权，对所有外部调用方开放。

| 接口 | 方法 | 说明 |
|------|------|------|
| [获取热门推荐游戏](recommended-games-top.md) | `POST /api/recommended-games/top` | 按策略过滤后返回热门推荐游戏列表 |
| [执行推荐游戏](recommended-games-execute.md) | `POST /api/recommended-games/execute` | 点击推荐游戏，记录聊天消息 |
| [AI 助手对话](assistant.md) | `POST /api/assistant` | 向 AI 助手发送消息并获取回复 |
| [获取用户历史消息](messages.md) | `POST /api/messages` | 获取指定时间之前的最近 N 条聊天记录 |
| [创建新会话](newsession.md) | `POST /api/newsession` | 为用户创建新的聊天会话 |

## 内部 API

详见 [内部 API](internal.md)。
