# 05 · `query_membership_subscriptions` — 会员等级、订阅与关联订单价格查询

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::membership::query_membership_subscriptions` |
| 返回类型 | `Result<Vec<MembershipSubscriptionRow>, sqlx::Error>` |
| 源文件 | [`services/membership.rs`](../../crates/hiveweb/src/services/membership.rs) |
| 生产调用方 | builtin [`query_balance`](../../crates/hiveweb/src/runtime/builtins/query_balance.rs) |
| 缓存包装 | `query_membership_subscriptions_cached`，已定义但当前生产代码**没有调用** |
| 缓存键 / TTL | 包装函数被调用时使用 `subscriptions:{user_id}` / 300 秒，并会缓存空数组 |

> 当前 `query_balance` 直接调用非缓存函数；它接收的 Redis 参数在这条路径中未使用。因此修改缓存 TTL 或删除 Redis key，不会影响当前线上会员订阅查询。

## SQL 原文

```sql
SELECT
    um.membership_level         AS membership_level,
    ml.level_name               AS level_name,
    um.membership_category      AS membership_category,
    CASE um.membership_category
        WHEN 'SUBSCRIPTION' THEN '连续订阅'
        WHEN 'ONE_TIME' THEN '单次购买'
        ELSE um.membership_category
    END                         AS membership_category_name,
    um.effective_start_time     AS effective_start_time,
    um.effective_end_time       AS effective_end_time,
    um.product_title            AS product_title,
    us.id                       AS subscription_id,
    CASE um.membership_category
        WHEN 'SUBSCRIPTION' THEN us.status
        ELSE null
    END                         AS subscription_status,
    CASE um.membership_category
        WHEN 'SUBSCRIPTION' THEN '生效中'
        ELSE null
    END                         AS subscription_status_name,
    CASE um.membership_category
        WHEN 'SUBSCRIPTION' THEN us.next_billing_time
        ELSE null
    END                         AS next_billing_time,
    us.auto_renew               AS auto_renew,
    us.payment_method           AS payment_method,
    us.start_time               AS subscription_start_time,
    us.end_time                 AS subscription_end_time,
    CASE um.membership_category
        WHEN 'SUBSCRIPTION' THEN o.order_price
        ELSE null
    END                         AS next_price
FROM (
    SELECT * FROM cc_user_membership
    WHERE user_id = ? AND effective_end_time > now()
) um
LEFT JOIN cc_membership_level ml
    ON um.membership_level = ml.level_code
LEFT JOIN (
    SELECT * FROM cc_user_subscription
    WHERE user_id = ? AND status = 'ACTIVE'
) us
    ON um.user_id = us.user_id AND um.product_id = us.product_id
LEFT JOIN (
    SELECT * FROM cc_subscription_order WHERE user_id = ?
) so
    ON so.user_subscription_id = us.id AND us.product_id = so.product_id
LEFT JOIN (
    SELECT * FROM cc_order WHERE user_id = ?
) o
    ON o.id = so.order_id AND o.asset_product_id = so.product_id
ORDER BY ml.level_order DESC, um.effective_end_time DESC
```

## 作用与查询链路

查询用户所有**尚未到期**的会员记录，并补充：

- `cc_membership_level` 中的等级名称和排序权重；
- 同一用户、同一商品下状态为 `ACTIVE` 的订阅；
- 订阅订单与资产订单；
- 资产订单的 `order_price`，映射到名为 `next_price` 的返回字段（单位：分）。

SQL 没有按订单状态、扣款时间或创建时间筛选，也没有只取一条订单，因此 `next_price` 只是当前关联到的订单价格，**不能保证代表下一次扣款金额**。

所有关联表均使用 `LEFT JOIN`，所以等级字典、订阅或订单缺失时，会员记录本身仍可返回。订阅不是按 `user_subscription_id` 直接关联，而是按 `user_id + product_id` 关联；订单链路还会校验 `product_id` / `asset_product_id`。

## 参数

函数签名为：

```rust
query_membership_subscriptions(ext_pool, user_id)
```

四个 SQL 占位符都绑定同一个 `user_id: i64`，顺序如下：

| 次序 | 过滤位置 |
| ---- | -------- |
| 1 | `cc_user_membership.user_id` |
| 2 | `cc_user_subscription.user_id` |
| 3 | `cc_subscription_order.user_id` |
| 4 | `cc_order.user_id` |

代码对应 `.bind(user_id)` 四次。

## 返回字段

| Rust 字段 | 来源 / 语义 |
| --------- | ----------- |
| `membership_level` | `um.membership_level` |
| `level_name` | `ml.level_name` |
| `membership_category` | `um.membership_category` |
| `membership_category_name` | `SUBSCRIPTION → 连续订阅`，`ONE_TIME → 单次购买` |
| `effective_start_time` | 会员开始时间；SQL 当前不按它过滤 |
| `effective_end_time` | 会员结束时间；只返回 `> now()` 的记录 |
| `product_title` | 会员记录中的商品标题 |
| `subscription_id` | 匹配到的 ACTIVE 订阅 ID |
| `subscription_status` | 仅连续订阅返回 `us.status`；当前子查询只允许 `ACTIVE` |
| `subscription_status_name` | 连续订阅固定返回 `生效中`，否则为 NULL |
| `next_billing_time` | 连续订阅的下一扣款时间 |
| `auto_renew` | 订阅自动续费标记；`query_balance` 输出时转成布尔值 |
| `payment_method` | 订阅支付方式 |
| `subscription_start_time` / `subscription_end_time` | 订阅合同起止时间 |
| `next_price` | 连续订阅关联订单的 `order_price`，单位为分；字段名虽为 `next_price`，SQL 不保证它是下一次扣款金额 |

结果按 `ml.level_order DESC, um.effective_end_time DESC` 排序。SQL 没有去重；如果一个订阅关联多条满足条件的订单，可能返回多行。

## `query_balance` 中的实际行为

`category = "discount"` 会在本查询前直接返回。其他类别只有在所需的金币、云硬盘等前置查询成功且没有提前返回时，才会执行本查询。返回行会被序列化到会员卡片中；如果查询结果为空，builtin 会把空数组替换为：

```json
[{ "level_name": "普通用户" }]
```

这是 builtin 的展示层兜底，不是本服务函数的返回值。

## 空结果与错误

- 没有 `effective_end_time > now()` 的会员记录：服务函数返回空 `Vec`。
- `effective_start_time` 在未来：当前 SQL **仍会返回**，因为这里只检查结束时间，没有检查 `effective_start_time <= now()`。
- ACTIVE 订阅未匹配：订阅和订单字段多数为 NULL；但 `subscription_status_name` 只根据会员类别判断，连续订阅仍会得到 `生效中`。
- 数据库查询失败：返回原始 `sqlx::Error`；`query_balance` 将其转为“会员订阅查询失败”。
