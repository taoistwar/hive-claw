# 01 · `check_vip_membership` — VIP 会员资格校验

## 元数据

| 字段 | 当前实现 |
| ---- | -------- |
| Rust 函数 | `services::membership::check_vip_membership` |
| 源文件 | [`services/membership.rs`](../../crates/hiveweb/src/services/membership.rs) |
| 外部表 | `cc_user_membership` |
| 当前调用方式 | 直接查询外部数据库 |
| Redis | **未使用**；当前不存在 `check_vip_membership_cached` |
| 主要调用方 | Assistant 对话、`GET /api/quota`、Legacy 推荐游戏兼容接口 |

## SQL 原文

```sql
SELECT id, membership_level, effective_end_time
FROM cc_user_membership
WHERE user_id = ? AND effective_end_time > ?
AND effective_start_time <= ? LIMIT 1
```

SQL 中没有 `NOW()`。Rust 在执行查询前只生成一次 `current_time`，然后把它同时绑定到结束时间和开始时间条件：

```text
current_time = Utc::now()
    -> 转换到固定 UTC+8
    -> 去除时区信息，得到 NaiveDateTime
```

## 参数

参数按绑定顺序如下：

| 顺序 | Rust 类型 | 绑定值 | SQL 用途 |
| ---- | --------- | ------ | -------- |
| 1 | `i64` | `user_id` | 匹配 `cc_user_membership.user_id` |
| 2 | `NaiveDateTime` | `current_time` | `effective_end_time > current_time` |
| 3 | `NaiveDateTime` | 同一个 `current_time` | `effective_start_time <= current_time` |

有效窗口采用左闭右开语义：

- 开始时间恰好等于当前时间时，会员已生效。
- 结束时间恰好等于当前时间时，会员已失效。
- `effective_start_time` 或 `effective_end_time` 为 `NULL` 时，比较结果不成立，该行不会命中。

## 返回

```rust
Result<bool, sqlx::Error>
```

- `fetch_optional` 命中任意一行时返回 `true`。
- 没有命中时返回 `false`。
- SQL 执行或字段解码失败时返回 `sqlx::Error`。

查询结果先映射为内部结构 `CcUserMembership`。`id`、`membership_level` 和 `effective_end_time` 用于调试日志；Rust 不会再对结束时间做第二次有效性判断。

## Redis 与调用行为

当前 VIP 状态每次都直接查询外部数据库。`cache_helper.rs` 中仍保留 `KEY_VIP_STATUS` 和 `TTL_VIP_STATUS` 常量，但生产代码没有对应缓存包装，也没有使用这两个常量执行读写。

主要调用方在查询失败时都会把用户降级为非 VIP，而不是使用一份过期的 VIP 缓存；错误日志行为并不相同：

- `api/chat_assistant.rs` 的对话接口和 `/api/quota` 会记录错误后降级。
- Legacy `api/recommended_game.rs` 的推荐游戏执行接口使用 `unwrap_or(false)` 静默降级，不记录这次会员查询错误；该调用点仅为兼容保留，新代码不得依赖。

## 时间约定

`current_time` 是 UTC+8 的本地墙上时间，但绑定类型是没有时区信息的 MySQL `DATETIME`。因此这条查询假设 `effective_start_time` 和 `effective_end_time` 也按 UTC+8 墙上时间保存。

使用应用时间而不是数据库 `NOW()`，可以避免应用实际连接到不同时区配置的 MySQL 实例时得到不同结果。若外部表实际按 UTC 保存，则应先统一数据约定，不能只调整 SQL。

## 边界与注意事项

- `LIMIT 1` 没有 `ORDER BY`；业务只需要判断“是否存在任意有效记录”，不依赖具体命中哪一条。
- 永久会员如果以 `effective_end_time = NULL` 表示，当前 SQL 会把它视为非会员。
- 日志会记录会员记录 ID、等级和结束时间，但不会把整行数据返回给接口调用方。
