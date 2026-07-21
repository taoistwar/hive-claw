# 06 · `query_duration_cards` — 有效金卡 / 黑金卡查询

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::membership::query_duration_cards` |
| 返回类型 | `Result<Vec<DurationCardRow>, sqlx::Error>` |
| 源文件 | [`services/membership.rs`](../../crates/hiveweb/src/services/membership.rs) |
| 生产调用方 | builtin [`query_balance`](../../crates/hiveweb/src/runtime/builtins/query_balance.rs)，仅 `duration_card` / `benefits` 类别 |
| 缓存包装 | `query_duration_cards_cached`，已定义但当前生产代码**没有调用** |
| 缓存键 / TTL | 包装函数被调用时使用 `duration_cards:{user_id}` / 300 秒，并会缓存空数组 |

> 当前 `query_balance` 直接查询外部数据库，没有走 `query_duration_cards_cached`。

## SQL 原文

```sql
SELECT
    t1.id                  AS card_asset_id,
    t1.value               AS remain_duration,
    t1.computer_biz_type   AS computer_biz_type,
    t1.expire_time         AS expire_time,
    t1.order_id            AS order_id,
    t1.consume_label       AS consume_label,
    t1.create_time         AS create_time,
    t4.game_label_list     AS game_label_list,
    t3.title               AS product_title,
    t3.value               AS product_duration
FROM (
    SELECT * FROM cc_user_asset_coin
    WHERE user_id = ?
      AND value > 0
      AND type IN (8, 9)
      AND (
        (type = 8 AND expire_time > UNIX_TIMESTAMP() * 1000)
        OR
        (type = 9 AND (expire_time IS NULL OR expire_time > UNIX_TIMESTAMP() * 1000))
      )
      AND (
        consume_label IS NULL
        OR (
          NOT JSON_CONTAINS(consume_label, '"FREE_CARD"', '$.gameLabelList')
          AND NOT JSON_CONTAINS(consume_label, '"BOX_CARD"', '$.gameLabelList')
          AND NOT JSON_CONTAINS(consume_label, '"BOX_CARD_MEMBER"', '$.gameLabelList')
        )
      )
) t1
LEFT JOIN (
    SELECT * FROM cc_order WHERE user_id = ?
) t2 ON t1.order_id = t2.id
LEFT JOIN cc_product t3 ON t2.asset_product_id = t3.id
LEFT JOIN cc_product_ext t4 ON t3.id = t4.product_id
```

## 作用与过滤规则

查询用户剩余值大于 0 的金卡和黑金卡资产，并沿订单关系补充商品名称、商品时长和适用游戏标签。

| 条件 | 含义 |
| ---- | ---- |
| `type = 8` | 金卡，必须具有晚于当前时间的毫秒级 `expire_time` |
| `type = 9` | 黑金卡，`expire_time IS NULL` 视为永久有效，否则必须尚未过期 |
| `value > 0` | 排除已经消耗完的卡 |
| `consume_label` 过滤 | 排除 `gameLabelList` 含 `FREE_CARD`、`BOX_CARD` 或 `BOX_CARD_MEMBER` 的资产 |

代码实际检查的是 `consume_label`，不是旧注释中提到的 `customer_label`。SQL 没有 `ORDER BY`，调用方不能依赖返回顺序。

## 参数

函数签名为：

```rust
query_duration_cards(ext_pool, user_id)
```

| 次序 | 类型 | 过滤位置 |
| ---- | ---- | -------- |
| 1 | `i64` | `cc_user_asset_coin.user_id` |
| 2 | `i64` | `cc_order.user_id` |

代码对应 `.bind(user_id).bind(user_id)`。

## 返回字段

结构体字段均为 `Option`：

| Rust 字段 | 来源 / 语义 |
| --------- | ----------- |
| `card_asset_id` | `cc_user_asset_coin.id` |
| `remain_duration` | `cc_user_asset_coin.value`，剩余时长数值 |
| `computer_biz_type` | 电脑业务类型 |
| `expire_time` | 毫秒时间戳；黑金卡可为 NULL |
| `order_id` | 关联订单 ID |
| `consume_label` | 资产消费标签 JSON |
| `create_time` | 资产创建时间 |
| `game_label_list` | `cc_product_ext.game_label_list` JSON |
| `product_title` | `cc_product.title`，例如金卡、黑金卡 |
| `product_duration` | `cc_product.value`，代码类型为 `Option<String>` |

旧文档中的 `card_type`、`card_type_name`、`extra` 和 `product_mirror` 已不在当前返回结构中。

## `query_balance` 中的实际行为

仅当 `category` 为 `duration_card` 或 `benefits` 时执行本查询。查询后，builtin 会：

1. 将卡片行序列化为 `duration_card` 数组；
2. 收集 `game_label_list` 中的代码；
3. 额外查询 `cc_label(value, name)`，把代码替换为中文名称；
4. 标签解析失败时只记录 warning，保留原始代码并继续返回。

## 空结果与错误

- 无满足条件的卡：服务函数返回空 `Vec`；builtin 输出空的 `duration_card` 数组，不生成专用错误消息。
- 订单、商品或商品扩展缺失：`LEFT JOIN` 保留资产行，对应补充字段为 NULL。
- 标签不存在于 `cc_label`：保留原标签代码。
- 数据库查询失败：返回 `sqlx::Error`；`query_balance` 将其转为“时长卡查询失败”。
