# 05 · `query_membership_subscriptions` — 会员等级 + 订阅合同 联合查询

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::membership::query_membership_subscriptions` |
| 缓存包装 | `query_membership_subscriptions_cached`（`cached_or_fetch`） |
| 缓存键 | `subscriptions:{user_id}` |
| 缓存 TTL | 300 s（5 min） |
| 源文件 | [services/membership.rs L134-174](../../crates/hiveweb/src/services/membership.rs#L134-L174) |
| 调用方 | builtin [`query_balance`](../../crates/hiveweb/src/runtime/builtins/query_balance.rs)（在「订阅信息」分支调用） |

## SQL 原文

```sql
SELECT
    um.membership_level         AS membership_level,
    ml.level_name               AS level_name,
    um.membership_category      AS membership_category,
    CASE um.membership_category
        WHEN 'SUBSCRIPTION' THEN '订阅型'
        WHEN 'ONE_TIME'    THEN '一次性'
        ELSE um.membership_category
    END                         AS membership_category_name,
    um.effective_start_time     AS effective_start_time,
    um.effective_end_time       AS effective_end_time,
    um.product_title            AS product_title,
    us.id                       AS subscription_id,
    us.status                   AS subscription_status,
    CASE us.status
        WHEN 'ACTIVE'  THEN '生效'
        WHEN 'REVOKE'  THEN '已解约'
        WHEN 'EXPIRED' THEN '已过期'
        WHEN 'PENDING' THEN '待签约'
        ELSE us.status
    END                         AS subscription_status_name,
    us.next_billing_time        AS next_billing_time,
    us.auto_renew               AS auto_renew,
    us.payment_method           AS payment_method,
    us.start_time               AS subscription_start_time,
    us.end_time                 AS subscription_end_time
FROM cc_user_membership um
LEFT JOIN cc_membership_level    ml ON um.membership_level       = ml.level_code
LEFT JOIN cc_user_subscription  us ON um.user_subscription_id  = us.id
WHERE um.user_id = ?
ORDER BY ml.level_order DESC, um.effective_end_time DESC
```

## 作用

返回某用户**所有会员记录**，并附带：

- 等级的中文名（`cc_membership_level.level_name`）
- 订阅合同的状态码 + 中文名
- 自动续费 / 支付方式 / 续费时间

`CASE` 块把状态码 / 分类码翻译成中文，**减轻前端 i18n 负担**。两个 `LEFT JOIN` 确保即便等级字典缺失或未绑定订阅，也能返回会员记录本身。

## 参数

| 占位符 | 类型 | 含义 |
| ------ | ---- | ---- |
| `?`    | `i64` | `cc_user_membership.user_id` |

## 返回

| Rust 类型 | 描述 |
| --------- | ---- |
| `Result<Vec<MembershipSubscriptionRow>, sqlx::Error>` | 数组，按 `level_order DESC, effective_end_time DESC` 排序（**最高等级、最晚到期 排第一**） |

## 涉及的表 / 列

| 表 | 关键列 |
| -- | ------ |
| `cc_user_membership` | `user_id`（过滤）/ `membership_level` / `membership_category` / `effective_start_time` / `effective_end_time` / `product_title` / `user_subscription_id` |
| `cc_membership_level` | `level_code`（JOIN 键）/ `level_name` / `level_order`（排序） |
| `cc_user_subscription` | `id`（JOIN 键）/ `status` / `next_billing_time` / `auto_renew` / `payment_method` / `start_time` / `end_time` |

## 排序约定

- 第一关键字 `ml.level_order DESC`：高级别会员排前
- 第二关键字 `um.effective_end_time DESC`：同级别按到期时间晚的排前

## 失败 / 边界

- 用户无任何会员记录 → 返回空 `Vec`。
- `ml` 或 `us` 缺失（孤立会员）→ 相关列返回 NULL，业务侧按"未知"展示。
