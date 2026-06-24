# 05 · `query_membership_subscriptions` — 会员等级 + 订阅合同 联合查询

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::membership::query_membership_subscriptions` |
| 缓存包装 | `query_membership_subscriptions_cached`（`cached_or_fetch`） |
| 缓存键 | `subscriptions:{user_id}` |
| 缓存 TTL | 300 s（5 min） |
| 源文件 | [services/membership.rs L135-179](../../crates/hiveweb/src/services/membership.rs#L135-L179) |
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
FROM (
    SELECT * FROM cc_user_membership
    WHERE user_id = ? AND effective_end_time > now()
) um
LEFT JOIN cc_membership_level ml ON um.membership_level = ml.level_code
LEFT JOIN (
    SELECT * FROM cc_user_subscription WHERE user_id = ?
) us ON um.user_subscription_id = us.id
ORDER BY ml.level_order DESC, um.effective_end_time DESC
```

## ⚠️ 最近变更

| 维度 | 旧 | 新 |
| ---- | -- | -- |
| `cc_user_membership` 来源 | 主表直查 | 改为子查询 `FROM (SELECT * FROM cc_user_membership WHERE user_id = ? AND effective_end_time > now()) um` |
| `cc_user_subscription` 来源 | 主表 LEFT JOIN | 改为子查询 `LEFT JOIN (SELECT * FROM cc_user_subscription WHERE user_id = ?) us` |
| `user_id = ?` 过滤 | 仅在外部 `WHERE um.user_id = ?` | 推到 `cc_user_membership` / `cc_user_subscription` 子查询内部 |
| 外部 WHERE 子句 | `WHERE um.user_id = ?` | 移除（用户过滤已下沉） |
| bind 数 | 1 | **2**（两个子查询各一） |

`effective_end_time > now()` 现在下推到 `cc_user_membership` 子查询，**与 `check_vip_membership` 的双时间过滤策略一致**（仅 `end > now()` 即可，子查询里的 `start_time` 暂未做 `< now()` 过滤——因为历史订阅可能仍有查询需求）。

`cc_user_subscription` 子查询加 `user_id = ?` 是性能优化——避免对全表的 LEFT JOIN。

## 作用

返回某用户**当前有效**的所有会员记录，并附带：

- 等级的中文名（`cc_membership_level.level_name`）
- 订阅合同的状态码 + 中文名
- 自动续费 / 支付方式 / 续费时间

`CASE` 块把状态码 / 分类码翻译成中文，**减轻前端 i18n 负担**。两个 `LEFT JOIN` 确保即便等级字典缺失或未绑定订阅，也能返回会员记录本身。

## 参数

| 占位符 | 类型 | 出现 | 含义 |
| ------ | ---- | ---- | ---- |
| `?`    | `i64` | 1 | `cc_user_membership.user_id` 子查询内 |
| `?`    | `i64` | 2 | `cc_user_subscription.user_id` 子查询内 |

代码里顺序为 `.bind(user_id).bind(user_id)`。

## 返回

| Rust 类型 | 描述 |
| --------- | ---- |
| `Result<Vec<MembershipSubscriptionRow>, sqlx::Error>` | 数组，按 `level_order DESC, effective_end_time DESC` 排序（**最高等级、最晚到期 排第一**） |

## 涉及的表 / 列

| 表 | 角色 | 关键列 |
| -- | ---- | ------ |
| `cc_user_membership` | 主表子查询 | `user_id`（过滤）/ `effective_end_time > now()`（过滤）/ `membership_level` / `membership_category` / `effective_start_time` / `effective_end_time` / `product_title` / `user_subscription_id` |
| `cc_membership_level` | 等级字典 | `level_code`（JOIN 键）/ `level_name` / `level_order`（排序） |
| `cc_user_subscription` | 订阅合同子查询 | `user_id`（过滤）/ `id`（JOIN 键）/ `status` / `next_billing_time` / `auto_renew` / `payment_method` / `start_time` / `end_time` |

## 排序约定

- 第一关键字 `ml.level_order DESC`：高级别会员排前
- 第二关键字 `um.effective_end_time DESC`：同级别按到期时间晚的排前

## 失败 / 边界

- 用户无任何**有效**会员记录（`effective_end_time <= now()`）→ 返回空 `Vec`。
- `ml` 或 `us` 缺失（孤立会员）→ 相关列返回 NULL，业务侧按"未知"展示。
- 失效会员（`end <= now()`）被 DB 预过滤，不再返回。
