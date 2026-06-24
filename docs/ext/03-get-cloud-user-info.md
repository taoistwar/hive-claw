# 03 · `get_cloud_user_info` — 取 cloud_user 业务身份 (uid, nickname)

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::membership::get_cloud_user_info` |
| 缓存包装 | `get_cloud_user_info_cached`（`cached_or_fetch`） |
| 缓存键 | `cloud_user:info:{user_id}` |
| 缓存 TTL | 900 s（15 min） |
| 源文件 | [services/membership.rs L46-56](../../crates/hiveweb/src/services/membership.rs#L46-L56) |
| 调用方 | [`api/chat_assistant.rs`](../../crates/hiveweb/src/api/chat_assistant.rs) `POST /assistant/send` 步骤 5 之后；[`api/newsession.rs`](../../crates/hiveweb/src/api/newsession.rs) `POST /assistant/session` 步骤 1 之后 |

## SQL 原文

```sql
SELECT uid, nickname FROM cloud_user WHERE ID = ?
```

## 作用

在确认 `user_id` 存在于 `cloud_user` 之后，再取**业务登录身份**（`uid` 是给前端展示的业务账号，`nickname` 是昵称），用于：

- 注入到 Agent 的 system context
- 透传回 Assistant API 调用方
- 写入 Langfuse trace 的 metadata

## 参数

| 占位符 | 类型 | 含义 |
| ------ | ---- | ---- |
| `?`    | `i64` | `cloud_user.ID` 主键 |

## 返回

| Rust 类型 | 描述 |
| --------- | ---- |
| `Result<Option<(String, String)>, sqlx::Error>` | `Some((uid, nickname))` 或 `None`（用户不存在） |

## 表结构

| 列 | 类型 | 用途 |
| -- | ---- | ---- |
| `ID` | `BIGINT` PK | 用户主键 |
| `uid` | `VARCHAR` | 业务登录账号 |
| `nickname` | `VARCHAR` | 昵称（可空，但 SQL 不会过滤） |

## 失败 / 边界

- 与 `user_exists_in_cloud` 配对使用：调用方先调用 02 确认存在，再调用 03 取身份；若 02 缓存为 false 时直接短路。
- `nickname` 为 NULL → 仍返回 `Some(...)` 元组，由调用方决定如何展示。
