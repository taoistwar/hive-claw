# 06 · `query_duration_cards` — 时长卡（金卡 / 黑金卡）查询

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::membership::query_duration_cards` |
| 缓存包装 | `query_duration_cards_cached`（`cached_or_fetch`） |
| 缓存键 | `duration_cards:{user_id}` |
| 缓存 TTL | 300 s（5 min） |
| 源文件 | [services/membership.rs L193-227](../../crates/hiveweb/src/services/membership.rs#L193-L227) |
| 调用方 | builtin [`query_balance`](../../crates/hiveweb/src/runtime/builtins/query_balance.rs)（在「时长卡」分支调用） |

## SQL 原文

```sql
SELECT
    uac.id                  AS card_asset_id,
    uac.value               AS remain_duration,
    uac.computer_biz_type   AS computer_biz_type,
    uac.expire_time         AS expire_time,
    uac.type                AS card_type,
    CASE uac.type
        WHEN 8 THEN '金卡'
        WHEN 9 THEN '黑金卡'
        ELSE '其他'
    END                     AS card_type_name,
    uac.order_id            AS order_id,
    uac.consume_label       AS consume_label,
    uac.extra               AS extra,
    uac.create_time         AS create_time
FROM cc_user_asset_coin uac
WHERE uac.user_id = ?
  AND uac.type IN (8, 9)
  AND uac.value > 0
  AND (
      (type = 8 AND expire_time > UNIX_TIMESTAMP() * 1000)
      OR
      (type = 9 AND (expire_time IS NULL OR expire_time > UNIX_TIMESTAMP() * 1000))
  )
ORDER BY uac.value ASC
```

## 作用

返回用户在外部库持有的**两类时长卡**（金卡 type=8、黑金卡 type=9）的**有效**记录。有效期判定规则不同：

- **金卡 (type=8)**：必须 `expire_time > now()`（硬过期）
- **黑金卡 (type=9)**：允许 `expire_time IS NULL`（永久）或 `expire_time > now()`

`value > 0` 过滤掉已消耗完的卡。

## 参数

| 占位符 | 类型 | 含义 |
| ------ | ---- | ---- |
| `?`    | `i64` | `cc_user_asset_coin.user_id` |

## 返回

| Rust 类型 | 描述 |
| --------- | ---- |
| `Result<Vec<DurationCardRow>, sqlx::Error>` | 时长卡数组，按 `value ASC` 排序（**剩余时长最短的优先消耗**） |

`DurationCardRow` 字段：

| Rust 字段 | 列 | 类型 |
| --------- | -- | ---- |
| `card_asset_id` | `id` | `i64` |
| `remain_duration` | `value` | `i64`（单位：分钟，外部库约定） |
| `computer_biz_type` | `computer_biz_type` | `String` |
| `expire_time` | `expire_time` | `i64`（毫秒戳） |
| `card_type` | `type` | `i8` |
| `card_type_name` | `CASE` 派生 | `String`（金卡 / 黑金卡 / 其他） |
| `order_id` | `order_id` | `i64` |
| `consume_label` | `consume_label` | `serde_json::Value`（JSON） |
| `extra` | `extra` | `serde_json::Value`（JSON） |
| `create_time` | `create_time` | `chrono::DateTime<Utc>` |

## 排序约定

`ORDER BY uac.value ASC`：剩余时长**短**的优先 → 业务侧按 FIFO 消耗，**避免长卡先被扣光造成浪费**。

## 失败 / 边界

- 用户没有任何时长卡 → 返回空 `Vec`。
- `expire_time` 已过期但 `value > 0` 的金卡 → 被 WHERE 过滤；黑金卡若 `expire_time IS NULL` → 永不过期，被保留。
