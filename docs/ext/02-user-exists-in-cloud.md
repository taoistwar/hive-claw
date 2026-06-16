# 02 · `user_exists_in_cloud` — 外部 cloud_user 存在性校验

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::membership::user_exists_in_cloud` |
| 缓存包装 | `user_exists_in_cloud_cached`（自定义 split-TTL） |
| 缓存键 | `cloud_user:exists:{user_id}` |
| 缓存 TTL | 存在 86400 s（24 h）/ 不存在 300 s（5 min） |
| 源文件 | [services/membership.rs L37-43](../../crates/hiveweb/src/services/membership.rs#L37-L43) |
| 调用方 | [`api/chat_assistant.rs`](../../crates/hiveweb/src/api/chat_assistant.rs) `POST /assistant/send` 步骤 5；[`api/newsession.rs`](../../crates/hiveweb/src/api/newsession.rs) `POST /assistant/session` 步骤 1 |

## SQL 原文

```sql
SELECT COUNT(1) FROM cloud_user WHERE ID = ?
```

## 作用

判断某个 `user_id` 是否已在外部 `cloud_computer.cloud_user` 表中注册过。Assistant API 在收到外部调用方请求时**先做这一步校验**——若该用户从未在云电脑侧开过户，则不进入业务流。

## 参数

| 占位符 | 类型 | 含义 |
| ------ | ---- | ---- |
| `?`    | `i64` | `cloud_user.ID` 主键 |

## 返回

| Rust 类型 | 描述 |
| --------- | ---- |
| `Result<bool, sqlx::Error>` | `true` = 用户存在，`false` = 不存在 |

> SQL 本身返回 `i64` 计数；函数用 `count > 0` 规整为 `bool`。

## 表结构

| 列 | 类型 | 用途 |
| -- | ---- | ---- |
| `ID` | `BIGINT` PK | 用户主键（注意大写） |

## 缓存策略说明

Split-TTL 是有意设计：

- **存在** → 缓存 24 h。账号在 cloud_user 中只会新增不会删除（删除走合规流程），长缓存大幅减轻外部库压力。
- **不存在** → 缓存 5 min。新注册用户应在 5 分钟内同步到外部库，过短会污染查询、过长会拒绝新用户。

## 失败 / 边界

- 外部库不可达 → `sqlx::Error` → 上层返回 `"Assistant service unavailable"`。
- `count = 0` 是正常分支，**不视为错误**。
