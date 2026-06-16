# 01 · `check_vip_membership` — VIP 会员资格校验

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::membership::check_vip_membership` |
| 缓存包装 | `check_vip_membership_cached`（`cached_or_fetch`） |
| 缓存键 | `vip:status:{user_id}` |
| 缓存 TTL | 300 s（5 min） |
| 源文件 | [services/membership.rs L21-34](../../crates/hiveweb/src/services/membership.rs#L21-L34) |
| 调用方 | [`api/chat_assistant.rs`](../../crates/hiveweb/src/api/chat_assistant.rs) `POST /assistant/send` 步骤 6 |

## SQL 原文

```sql
SELECT id, membership_level, effective_end_time
FROM cc_user_membership
WHERE id = ?
LIMIT 1
```

> 注：表中 `id` 实际是 `cc_user_membership` 表的主键，对应 `user_id` 的最近一条会员记录。
> 该 SQL 之所以用 `id = ?`（而不是 `user_id = ?`）是外部库的历史 schema 设计：原表以 `id` 作为业务单条会员记录 ID，调用方传入的 `user_id` 需先经上游解析为具体记录 ID。

## 作用

判断指定用户**当前是否持有有效 VIP 会员**。逻辑：

- 无记录 → `false`
- 有记录且 `effective_end_time IS NULL` → `true`（视为永久）
- 有记录且 `effective_end_time >= now()` → `true`
- 其他 → `false`

## 参数

| 占位符 | 类型 | 含义 |
| ------ | ---- | ---- |
| `?`    | `i64` | 外部 `cc_user_membership.id`（与 `user_id` 1:1 的最近一条记录主键） |

## 返回

| Rust 类型 | 描述 |
| --------- | ---- |
| `Result<bool, sqlx::Error>` | `true` = 当前有效 VIP，`false` = 无效或无记录 |

映射到结构体 `CcUserMembership { id, membership_level, effective_end_time }` 后再在 Rust 侧判断有效期。

## 表结构（推断）

| 列 | 类型 | 用途 |
| -- | ---- | ---- |
| `id` | `BIGINT` PK | 单条会员记录主键 |
| `membership_level` | `VARCHAR` | 等级 code（外联 `cc_membership_level`） |
| `effective_end_time` | `DATETIME` | 生效截止时间（NULL = 永久） |

## 失败 / 边界

- 外部库不可达 → `sqlx::Error` → 上层返回 `"Assistant service unavailable"`。
- 记录存在但 `effective_end_time` 已过期 → 正常返回 `false`，不报错。
