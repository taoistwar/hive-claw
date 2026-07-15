# 03 · `get_cloud_user_info` — 查询云电脑用户身份

## 元数据

| 字段 | 当前实现 |
| ---- | -------- |
| Rust 函数 | `services::membership::get_cloud_user_info` |
| 缓存包装 | `services::membership::get_cloud_user_info_cached` |
| 源文件 | [`services/membership.rs`](../../crates/hiveweb/src/services/membership.rs) |
| 外部表 | `cloud_user` |
| 缓存键 | `cloud_user:info:{user_id}` |
| 缓存 TTL | 900 秒（15 分钟） |
| 缓存状态 | **生产调用链已启用** |

## SQL 原文

```sql
SELECT uid, nickname FROM cloud_user WHERE ID = ?
```

## 作用

一次查询同时完成两件事：

1. 用 `fetch_optional` 判断 `cloud_user.ID` 是否存在。
2. 命中时取得 `uid` 和 `nickname`，用于同步 hiveweb 自有 `users` 表中的用户身份。

原来的 `user_exists_in_cloud` / `user_exists_in_cloud_cached` 已删除。当前调用方不会先执行 `SELECT COUNT(1)`，而是直接按以下规则判断：

- `Some((uid, nickname))`：用户存在。
- `None`：用户不存在。

这避免了“先查是否存在，再查用户信息”的重复外部数据库请求。

## 参数

| 顺序 | Rust 类型 | 含义 |
| ---- | --------- | ---- |
| 1 | `i64` | `cloud_user.ID` 主键 |

SQL 函数本身不校验正数。Assistant 对话、newsession 和 quota 入口会先验证 `user_id > 0`；Legacy 推荐游戏执行接口只验证它可解析为 `i64`，没有额外拒绝零或负数。

## 返回

DB-only 函数：

```rust
Result<Option<(String, String)>, sqlx::Error>
```

缓存包装：

```rust
Result<Option<(String, String)>, String>
```

元组顺序固定为 `(uid, nickname)`。

结构映射使用 `(String, String)`，不是 `(Option<String>, Option<String>)`。因此只要命中行中的 `uid` 或 `nickname` 为 SQL `NULL`，SQLx 就会返回字段解码错误，而不会返回含空值的元组。

## 实际调用方

`get_cloud_user_info_cached` 当前用于：

- Assistant 对话入口：校验用户存在，并把 `uid`、`nickname` 同步到 hiveweb 用户表。
- `POST /api/newsession`：校验用户存在并同步用户信息后创建新会话。
- `GET /api/quota`：只使用 `Some` / `None` 做存在性校验。
- Legacy 推荐游戏接口：尽力读取用户信息并同步 hiveweb 用户表；该兼容调用点允许查询失败后继续使用空身份，新代码不得依赖。

## Redis cache-aside 行为

缓存包装调用通用 `cached_or_fetch`：

1. 从 `cloud_user:info:{user_id}` 读取 JSON。
2. 命中时直接反序列化并返回。
3. 未命中或 Redis 读失败时查询外部数据库。
4. 外部数据库查询成功后，把整个 `Option<(String, String)>` 写入 Redis，TTL 为 900 秒。

这里不仅缓存 `Some`，也会缓存 `None`，所以不存在的用户会被负缓存最多 15 分钟。它不再使用旧实现中“存在 24 小时、不存在 5 分钟”的 split-TTL。

Redis 连接、GET、反序列化或 SETEX 失败均按 best-effort 处理：读失败会回源数据库，写失败不会让本次 DB 查询失败。只有外部数据库查询失败才会让包装返回 `Err(String)`。

当前没有主动删除 `cloud_user:info:{user_id}` 的业务调用。外部库中的 `uid` 或 `nickname` 更新后，hiveweb 最多可能继续读到 15 分钟旧值；新注册用户也可能在负缓存到期前仍被判断为不存在。

## 边界与注意事项

- `ID` 是查询键，注意外部表列名使用大写形式。
- SQL 没有 `LIMIT 1`，但 `ID` 应是唯一键；`fetch_optional` 若遇到多行，会读取第一行，不会主动验证唯一性。
- 外部数据库未配置时，HTTP 调用方在执行本函数前按各自接口语义返回服务不可用错误。
