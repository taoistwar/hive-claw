# 04 · `query_membership_balance` — 余额 / 7 日内到期金币 / 云盘配额

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::membership::query_membership_balance` |
| 缓存包装 | `query_membership_balance_cached`（`cached_or_fetch`） |
| 缓存键 | `balance:{user_id}` |
| 缓存 TTL | 60 s（1 min，最短，因余额频繁变动） |
| 源文件 | [services/membership.rs L71-110](../../crates/hiveweb/src/services/membership.rs#L71-L110) |
| 调用方 | builtin [`query_balance`](../../crates/hiveweb/src/runtime/builtins/query_balance.rs)（Agent 在会话中调用以展示资产） |

## SQL 原文

```sql
select
    m7.total_coins, m2.expire_coins_7d, m4.disk_total_size, m4.disk_end_time
from
(
    select ? as user_id
) m1
left join
(
    select user_id, IFNULL(sum(value), 0) as expire_coins_7d from cc_user_asset_coin
    where user_id = ?
        and expire_time > UNIX_TIMESTAMP() * 1000
        and expire_time < (7*24*60*60*1000 + UNIX_TIMESTAMP() * 1000)
        and value > 0
    group by user_id
) m2 on m1.user_id = m2.user_id
LEFT JOIN (
    SELECT user_id, sum(size/1024/1024/1024) as disk_total_size, MAX(end_time) as disk_end_time
    from cc_user_disk
    where user_id = ? and end_time > UNIX_TIMESTAMP() * 1000 and start_time < UNIX_TIMESTAMP() * 1000
    group by user_id
) m4 on m1.user_id = m4.user_id
LEFT JOIN (
    select user_id, IFNULL(sum(value), 0) as total_coins from cc_user_asset_coin
    where user_id = ? and expire_time > UNIX_TIMESTAMP() and value > 0
    group by user_id
) m7 on m1.user_id = m7.user_id
```

## 作用

为 Agent builtin `query_balance` 一次性返回四个聚合指标：

| 字段 | 含义 |
| ---- | ---- |
| `total_coins` | 用户**未过期**金币余额合计（`expire_time` 折算到秒） |
| `expire_coins_7d` | **未来 7 天内**会过期的金币合计 |
| `disk_total_size` | 当前生效云盘配额合计（GB） |
| `disk_end_time` | 配额包最大到期时间（毫秒戳） |

## 参数

| 占位符 | 类型 | 出现次数 | 含义 |
| ------ | ---- | -------- | ---- |
| `?`    | `i64` | 4 次 | 同一 `user_id`，分别注入 `m1` 驱动表 + 三个聚合子查询 |

`m1` 是「驱动表」，用 `select ? as user_id` 强制即便用户无任何记录也能返回 1 行 NULL，再 LEFT JOIN 三个聚合子查询，保证**字段永远存在**（NULL 表示无该指标）。

## 返回

| Rust 类型 | 描述 |
| --------- | ---- |
| `Result<Option<MembershipBalanceRow>, sqlx::Error>` | `MembershipBalanceRow` 含 `total_coins` / `expire_coins_7d`（`Option<Decimal>`）+ `disk_total_size`（`Option<Decimal>`）+ `disk_end_time`（`Option<i64>`） |

## 涉及的表 / 列

| 表 | 列 | 用途 |
| -- | -- | ---- |
| `cc_user_asset_coin` | `user_id`, `value`, `expire_time` | 计算 `total_coins` 与 `expire_coins_7d` |
| `cc_user_disk` | `user_id`, `size`, `start_time`, `end_time` | 计算云盘配额 |

## 关键约定

- `cc_user_asset_coin.expire_time` / `cc_user_disk.{start,end}_time` 存的是**毫秒 BIGINT**，所以比较时是 `UNIX_TIMESTAMP() * 1000`；但 `total_coins` 子查询用 `UNIX_TIMESTAMP()`（秒）——这看起来是历史 SQL 写法遗留，**保留**。
- `size/1024/1024/1024` 把 byte 转 GB。
- 三个 LEFT JOIN 保证字段**始终**存在，NULL → 调用方解读为「无该项资产」。

## 失败 / 边界

- 用户无任何记录 → 返回一行全 NULL。
- 任一子查询失败 → 整条 SQL 失败 → `sqlx::Error`。
