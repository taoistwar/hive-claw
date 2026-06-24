# 06 · `query_duration_cards` — 时长卡（金卡 / 黑金卡）查询

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::membership::query_duration_cards` |
| 缓存包装 | `query_duration_cards_cached`（`cached_or_fetch`） |
| 缓存键 | `duration_cards:{user_id}` |
| 缓存 TTL | 300 s（5 min） |
| 源文件 | [services/membership.rs L200-239](../../crates/hiveweb/src/services/membership.rs#L200-L239) |
| 调用方 | builtin [`query_balance`](../../crates/hiveweb/src/runtime/builtins/query_balance.rs)（在「时长卡」分支调用） |

## SQL 原文

```sql
SELECT
    t1.id                  AS card_asset_id,
    t1.value               AS remain_duration,
    t1.computer_biz_type   AS computer_biz_type,
    t1.expire_time         AS expire_time,
    t1.type                AS card_type,
    CASE t1.type
        WHEN 8 THEN '金卡'
        WHEN 9 THEN '黑金卡'
        ELSE '其他'
    END                     AS card_type_name,
    t1.order_id            AS order_id,
    t1.consume_label       AS consume_label,
    t1.extra               AS extra,
    t1.create_time         AS create_time,
    t2.product_mirror      AS product_mirror
FROM (
    SELECT * FROM cc_user_asset_coin
    WHERE user_id = ?
      AND type IN (8, 9)
      AND value > 0
      AND (
          (type = 8 AND expire_time > UNIX_TIMESTAMP() * 1000)
          OR
          (type = 9 AND (expire_time IS NULL OR expire_time > UNIX_TIMESTAMP() * 1000))
      )
) t1
LEFT JOIN (
    SELECT * FROM cc_order WHERE user_id = ?
) t2 ON t1.order_id = t2.id
```

## ⚠️ 最近变更

| 维度 | 旧 | 新 |
| ---- | -- | -- |
| 主表别名 | `uac` | `t1`（从子查询里取） |
| `product_mirror` 来源 | `uac.product_mirror`（`cc_user_asset_coin`） | **`t2.product_mirror`（`cc_order`）** |
| 表结构 | 直接 `FROM cc_user_asset_coin` | `FROM (SELECT * FROM cc_user_asset_coin WHERE …) t1 LEFT JOIN (SELECT * FROM cc_order WHERE user_id = ?) t2 ON t1.order_id = t2.id` |
| `ORDER BY` | `ORDER BY uac.value ASC`（FIFO 消耗） | **删除**（无排序保证） |
| bind 数 | 1 | **2**（`cc_user_asset_coin` 子查询 1 + `cc_order` 子查询 1） |

- **`product_mirror` 现在来自 `cc_order` 表**，而不是 `cc_user_asset_coin`。这是个重要的语义变化：产品规格信息实际存储在订单表中，资产表只持有订单 ID 的引用。
- **删除了 `ORDER BY uac.value ASC`**。这意味着结果集顺序不再保证 FIFO，业务侧若需要按剩余时长消耗需在 Rust 端重排。
- 函数尾部有源代码注释提醒（未实现）：`// 排除 t1.customer_label "gameLabelList" 包含 "BOX_CARD"` —— 后续可能增加对 `customer_label` 的 JSON 字段过滤。

## 作用

返回用户在外部库持有的**两类时长卡**（金卡 type=8、黑金卡 type=9）的**有效**记录及其对应订单的产品规格。有效期判定规则不同：

- **金卡 (type=8)**：必须 `expire_time > now()`（硬过期）
- **黑金卡 (type=9)**：允许 `expire_time IS NULL`（永久）或 `expire_time > now()`

`value > 0` 过滤掉已消耗完的卡。`LEFT JOIN cc_order` 把订单的产品镜像（`product_mirror` JSON）一并带回，下游可从中提取 fps / gpu 等规格。

## 参数

| 占位符 | 类型 | 出现 | 含义 |
| ------ | ---- | ---- | ---- |
| `?`    | `i64` | 1 | `cc_user_asset_coin.user_id`（子查询内） |
| `?`    | `i64` | 2 | `cc_order.user_id`（子查询内） |

## 返回

| Rust 类型 | 描述 |
| --------- | ---- |
| `Result<Vec<DurationCardRow>, sqlx::Error>` | 时长卡数组（**顺序未定义**） |

`DurationCardRow` 字段：

| Rust 字段 | 列来源 | 类型 |
| --------- | ------ | ---- |
| `card_asset_id` | `t1.id` | `i64` |
| `remain_duration` | `t1.value` | `i64`（单位：分钟，外部库约定） |
| `computer_biz_type` | `t1.computer_biz_type` | `String` |
| `expire_time` | `t1.expire_time` | `i64`（毫秒戳） |
| `card_type` | `t1.type` | `i8` |
| `card_type_name` | `CASE` 派生 | `String`（金卡 / 黑金卡 / 其他） |
| `order_id` | `t1.order_id` | `i64` |
| `consume_label` | `t1.consume_label` | `serde_json::Value`（JSON） |
| `extra` | `t1.extra` | `serde_json::Value`（JSON） |
| `create_time` | `t1.create_time` | `chrono::DateTime<Utc>` |
| `product_mirror` | `t2.product_mirror`（**cc_order**） | `serde_json::Value`（JSON，提取 fps / gpu 等字段） |

## 失败 / 边界

- 用户无任何时长卡 → 返回空 `Vec`。
- `expire_time` 已过期但 `value > 0` 的金卡 → 被 WHERE 过滤；黑金卡若 `expire_time IS NULL` → 永不过期，被保留。
- `cc_order` 缺失（订单被删除 / 关联丢失）→ LEFT JOIN 保留 `t1` 资产行，`product_mirror = NULL`。
