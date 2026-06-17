# 01 · `check_vip_membership` — VIP 会员资格校验

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::membership::check_vip_membership` |
| 缓存包装 | `check_vip_membership_cached`（`cached_or_fetch`） |
| 缓存键 | `vip:status:{user_id}` |
| 缓存 TTL | 300 s（5 min） |
| 源文件 | [services/membership.rs L21-35](../../crates/hiveweb/src/services/membership.rs#L21-L35) |
| 调用方 | [`api/chat_assistant.rs`](../../crates/hiveweb/src/api/chat_assistant.rs) `POST /assistant/send` 步骤 6 |

## SQL 原文

```sql
SELECT id, membership_level, effective_end_time
FROM cc_user_membership
WHERE user_id = ?
  AND effective_end_time > now()
  AND effective_start_time < now()
LIMIT 1
```

## ⚠️ 最近变更

| 维度 | 旧 | 新 |
| ---- | -- | -- |
| WHERE 键 | `id = ?` | `user_id = ?` |
| 时间过滤 | 无（由 Rust 端判断） | DB 侧 `effective_end_time > now() AND effective_start_time < now()` |
| Rust 端处理 | 把 `effective_end_time` 与 `now()` 比较后才决定 bool | 直接看 `Option<row>` 决定 bool |

旧 SQL 用 `id = ?` 是历史误用（应传 `user_id` 时传了记录主键），同时把所有时间有效性判断放在 Rust 端。新 SQL 修正为 `user_id = ?`，并把"当前是否在生效窗口内"推到 DB 侧预过滤——减少回传行数，也让 `LIMIT 1` 配合"任意一行当前有效"语义更稳定。

## 作用

判断指定用户**当前是否持有有效 VIP 会员**。逻辑：

- DB 侧已预过滤 `effective_end_time > now() AND effective_start_time < now()`，未命中 → 返回 `Option::None` → `false`
- 命中 → Rust 端再检查 `effective_end_time`（冗余判断，防御 DB 时区差异）
  - `effective_end_time IS NULL` → `true`（视为永久，但此 SQL 已要求 > now()，实际上 NULL 行已被 DB 过滤；保留 Rust 兜底以防数据漂移）
  - `effective_end_time >= now()` → `true`
  - 其他 → `false`

## 参数

| 占位符 | 类型 | 含义 |
| ------ | ---- | ---- |
| `?`    | `i64` | `cc_user_membership.user_id` |

## 返回

| Rust 类型 | 描述 |
| --------- | ---- |
| `Result<bool, sqlx::Error>` | `true` = 当前有效 VIP，`false` = 无效或无记录 |

映射到结构体 `CcUserMembership { id, membership_level, effective_end_time }` 后再在 Rust 侧判断有效期。

## 表结构

| 列 | 类型 | 用途 |
| -- | ---- | ---- |
| `id` | `BIGINT` PK | 单条会员记录主键 |
| `user_id` | `BIGINT` | 所属用户（**本次变更后**作为查询键） |
| `membership_level` | `VARCHAR` | 等级 code（外联 `cc_membership_level`） |
| `effective_start_time` | `DATETIME` | 生效起始时间（**新增**过滤） |
| `effective_end_time` | `DATETIME` | 生效截止时间（NULL 视为永久；DB 过滤与 Rust 兜底并存） |

## 失败 / 边界

- 外部库不可达 → `sqlx::Error` → 上层返回 `"Assistant service unavailable"`。
- 记录存在但 `effective_end_time` 已过期 / 未到 `effective_start_time` → DB 预过滤为 0 行 → `false`。
- 时区差异（DB 与 Rust `Utc::now()` 错位） → Rust 端再次校验 `end >= Utc::now()` 兜底。
