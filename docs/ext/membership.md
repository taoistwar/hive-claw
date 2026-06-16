# 外部数据库 — 用户与会员

以下 SQL 查询对外部数据库中用户、会员、资产相关表。所有查询均为只读。

查询实现位于 [services/membership.rs](../../crates/hiveweb/src/services/membership.rs)。

---

## cloud_user — 云用户表

### 检查用户是否存在

```sql
SELECT COUNT(1) FROM cloud_user WHERE ID = ?
```

- **调用方：** `user_exists_in_cloud()`
- **返回：** `bool`
- **Redis 缓存：** `cloud_user:exists:{user_id}` — 存在 24h，不存在 5min

### 获取用户 uid 和昵称

```sql
SELECT uid, nickname FROM cloud_user WHERE ID = ?
```

- **调用方：** `get_cloud_user_info()`
- **返回：** `Option<(uid: String, nickname: String)>`
- **Redis 缓存：** `cloud_user:info:{user_id}` — TTL 15min

---

## cc_user_membership — 用户会员表

### 检查 VIP 状态

```sql
SELECT id, membership_level, effective_end_time
FROM cc_user_membership
WHERE id = ?
LIMIT 1
```

- **调用方：** `check_vip_membership()`
- **逻辑：** `effective_end_time >= now()` 视为有效 VIP
- **返回：** `bool`
- **Redis 缓存：** `vip:status:{user_id}` — TTL 5min

### 查询会员等级与订阅状态

```sql
SELECT
    um.membership_level         AS membership_level,
    ml.level_name               AS level_name,
    um.membership_category      AS membership_category,
    CASE um.membership_category
        WHEN 'SUBSCRIPTION' THEN '订阅型'
        WHEN 'ONE_TIME' THEN '一次性'
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
LEFT JOIN cc_membership_level ml ON um.membership_level = ml.level_code
LEFT JOIN cc_user_subscription us ON um.user_subscription_id = us.id
WHERE um.user_id = ?
ORDER BY ml.level_order DESC, um.effective_end_time DESC
```

- **调用方：** `query_membership_subscriptions()`
- **返回：** `Vec<MembershipSubscriptionRow>` — 14 个字段
- **Redis 缓存：** `subscriptions:{user_id}` — TTL 5min

### 返回行结构 `MembershipSubscriptionRow`

| 字段 | 类型 | 来源表 | 说明 |
|------|------|--------|------|
| `membership_level` | `Option<String>` | `cc_user_membership` | 会员等级编码 |
| `level_name` | `Option<String>` | `cc_membership_level` | 等级显示名称 |
| `membership_category` | `Option<String>` | `cc_user_membership` | `SUBSCRIPTION` / `ONE_TIME` |
| `membership_category_name` | `Option<String>` | CASE 派生 | 订阅型 / 一次性 |
| `effective_start_time` | `Option<NaiveDateTime>` | `cc_user_membership` | 生效开始时间 |
| `effective_end_time` | `Option<NaiveDateTime>` | `cc_user_membership` | 生效截止时间 |
| `product_title` | `Option<String>` | `cc_user_membership` | 商品标题 |
| `subscription_id` | `Option<i64>` | `cc_user_subscription` | 订阅 ID |
| `subscription_status` | `Option<String>` | `cc_user_subscription` | ACTIVE / REVOKE / EXPIRED / PENDING |
| `subscription_status_name` | `Option<String>` | CASE 派生 | 生效 / 已解约 / 已过期 / 待签约 |
| `next_billing_time` | `Option<NaiveDateTime>` | `cc_user_subscription` | 下次扣费时间 |
| `auto_renew` | `Option<i8>` | `cc_user_subscription` | 是否自动续费 |
| `payment_method` | `Option<String>` | `cc_user_subscription` | 支付方式 |
| `subscription_start_time` | `Option<NaiveDateTime>` | `cc_user_subscription` | 订阅开始时间 |
| `subscription_end_time` | `Option<NaiveDateTime>` | `cc_user_subscription` | 订阅结束时间 |

---

## cc_user_asset_coin — 用户金币资产表

### 查询余额（金币 + 7天内过期金币）

```sql
SELECT
    m7.total_coins,
    m2.expire_coins_7d,
    m4.disk_total_size,
    m4.disk_end_time
FROM (SELECT ? AS user_id) m1
LEFT JOIN (
    SELECT user_id, IFNULL(SUM(value), 0) AS expire_coins_7d
    FROM cc_user_asset_coin
    WHERE user_id = ?
        AND expire_time > UNIX_TIMESTAMP() * 1000
        AND expire_time < (7 * 24 * 60 * 60 * 1000 + UNIX_TIMESTAMP() * 1000)
        AND value > 0
    GROUP BY user_id
) m2 ON m1.user_id = m2.user_id
LEFT JOIN (
    SELECT user_id, SUM(size / 1024 / 1024 / 1024) AS disk_total_size,
        MAX(end_time) AS disk_end_time
    FROM cc_user_disk
    WHERE user_id = ?
        AND end_time > UNIX_TIMESTAMP() * 1000
        AND start_time < UNIX_TIMESTAMP() * 1000
    GROUP BY user_id
) m4 ON m1.user_id = m4.user_id
LEFT JOIN (
    SELECT user_id, IFNULL(SUM(value), 0) AS total_coins
    FROM cc_user_asset_coin
    WHERE user_id = ?
        AND expire_time > UNIX_TIMESTAMP()
        AND value > 0
    GROUP BY user_id
) m7 ON m1.user_id = m7.user_id
```

- **调用方：** `query_membership_balance()`
- **返回：** `Option<MembershipBalanceRow>` — 4 个字段
- **Redis 缓存：** `balance:{user_id}` — TTL 1min

### 返回行结构 `MembershipBalanceRow`

| 字段 | 类型 | 说明 |
|------|------|------|
| `total_coins` | `Option<Decimal>` | 总金币数（所有未过期金币） |
| `expire_coins_7d` | `Option<Decimal>` | 7 天内即将过期的金币数 |
| `disk_total_size` | `Option<Decimal>` | 网盘总大小（GB） |
| `disk_end_time` | `Option<i64>` | 网盘截止时间（Unix 时间戳 ms） |

### 查询时长卡（金卡 type=8 / 黑金卡 type=9）

```sql
SELECT
    uac.id                      AS card_asset_id,
    uac.value                   AS remain_duration,
    uac.computer_biz_type       AS computer_biz_type,
    uac.expire_time             AS expire_time,
    uac.type                    AS card_type,
    CASE uac.type
        WHEN 8 THEN '金卡'
        WHEN 9 THEN '黑金卡'
        ELSE '其他'
    END                         AS card_type_name,
    uac.order_id                AS order_id,
    uac.consume_label           AS consume_label,
    uac.extra                   AS extra,
    uac.create_time             AS create_time
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

- **调用方：** `query_duration_cards()`
- **返回：** `Vec<DurationCardRow>` — 10 个字段
- **Redis 缓存：** `duration_cards:{user_id}` — TTL 5min

### 返回行结构 `DurationCardRow`

| 字段 | 类型 | 说明 |
|------|------|------|
| `card_asset_id` | `Option<i64>` | 卡资产 ID（`cc_user_asset_coin.id`） |
| `remain_duration` | `Option<i64>` | 剩余时长（`value` 字段，秒） |
| `computer_biz_type` | `Option<String>` | 计算业务类型 |
| `expire_time` | `Option<i64>` | 过期时间（Unix 时间戳 ms） |
| `card_type` | `Option<i8>` | 卡类型：8=金卡，9=黑金卡 |
| `card_type_name` | `Option<String>` | 卡类型显示名：金卡 / 黑金卡 / 其他 |
| `order_id` | `Option<i64>` | 订单 ID |
| `consume_label` | `Option<Value>` | 消费标签（JSON） |
| `extra` | `Option<Value>` | 额外信息（JSON） |
| `create_time` | `Option<DateTime<Utc>>` | 创建时间 |

---

## cc_user_disk — 用户网盘表

网盘查询已内嵌在 `query_membership_balance()` 的 LEFT JOIN 中，无独立的查询函数。

```sql
SELECT user_id, SUM(size / 1024 / 1024 / 1024) AS disk_total_size,
    MAX(end_time) AS disk_end_time
FROM cc_user_disk
WHERE user_id = ?
    AND end_time > UNIX_TIMESTAMP() * 1000
    AND start_time < UNIX_TIMESTAMP() * 1000
GROUP BY user_id
```

- **含义：** 查询用户当前有效（start_time < now < end_time）的所有网盘记录，汇总总大小和最近截止时间。

---

## cc_membership_level — 会员等级定义表

通过 `LEFT JOIN` 在 `query_membership_subscriptions()` 中关联：

```sql
LEFT JOIN cc_membership_level ml ON um.membership_level = ml.level_code
```

| 字段 | 说明 |
|------|------|
| `level_code` | 等级编码（与 `cc_user_membership.membership_level` 关联） |
| `level_name` | 等级显示名称 |
| `level_order` | 排序权重（DESC 排序用） |

---

## cc_user_subscription — 用户订阅表

通过 `LEFT JOIN` 在 `query_membership_subscriptions()` 中关联：

```sql
LEFT JOIN cc_user_subscription us ON um.user_subscription_id = us.id
```

| 字段 | 说明 |
|------|------|
| `id` | 订阅 ID |
| `status` | ACTIVE / REVOKE / EXPIRED / PENDING |
| `next_billing_time` | 下次扣费时间 |
| `auto_renew` | 是否自动续费 |
| `payment_method` | 支付方式 |
| `start_time` | 订阅开始时间 |
| `end_time` | 订阅结束时间 |
