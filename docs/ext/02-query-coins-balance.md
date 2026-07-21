# 02 · `query_coins_balance` — 金币余额与 7 日内到期金币

## 元数据

| 字段 | 当前实现 |
| ---- | -------- |
| Rust 函数 | `services::membership::query_coins_balance` |
| 源文件 | [`services/membership.rs`](../../crates/hiveweb/src/services/membership.rs) |
| 外部表 | `cc_user_asset_coin` |
| 当前调用方 | builtin [`query_balance`](../../crates/hiveweb/src/runtime/builtins/query_balance.rs)，`category=coins` 或 `category=benefits` |
| 当前调用方式 | 直接查询外部数据库 |
| Redis 包装 | `query_coins_balance_cached` 已定义，但当前生产调用链**没有调用它** |
| 未使用缓存键 / TTL | `balance:coins:{user_id}` / 60 秒 |

`query_balance_async_impl` 的 Redis 参数目前命名为 `_redis`，金币查询直接调用 `query_coins_balance`。因此不能根据缓存包装函数和常量的存在推断线上正在缓存余额。

## SQL 原文

```sql
select
    m7.total_coins, m2.expire_coins_7d
from
(
    select ? as user_id
) m1
left join
(
    select user_id, IFNULL(sum(value), 0) as expire_coins_7d from cc_user_asset_coin
    where user_id = ?
        and expire_time > UNIX_TIMESTAMP() *1000
        and expire_time < (7*24*60*60*1000+UNIX_TIMESTAMP()*1000)
        and value>0 AND type IN (0,1,2,3,4,5,6,7,10,12,13,17,18,19,20)
    group by user_id
) m2 on m1.user_id = m2.user_id
LEFT JOIN (
    select user_id, IFNULL(sum(value), 0) as total_coins from cc_user_asset_coin
    where user_id = ? and expire_time > UNIX_TIMESTAMP() and value>0 AND type IN (0,1,2,3,4,5,6,7,10,12,13,17,18,19,20)
    group by user_id
) m7 on m1.user_id = m7.user_id
```

## 统计口径

两个聚合都只统计：

- `value > 0` 的资产。
- `type IN (0,1,2,3,4,5,6,7,10,12,13,17,18,19,20)` 的资产。

返回字段：

| 字段 | 统计方式 |
| ---- | -------- |
| `expire_coins_7d` | 当前时刻之后、严格早于未来 7 天的金币合计 |
| `total_coins` | 按 `m7` 子查询中的到期时间条件统计的金币合计 |

类型 8、9 不在范围内；它们由时长卡查询处理。当前实现也不是“仅统计 type=4”。

## 参数

三个占位符都绑定同一个 `user_id`：

| 顺序 | Rust 类型 | SQL 用途 |
| ---- | --------- | -------- |
| 1 | `i64` | 构造 `m1` 驱动行 |
| 2 | `i64` | 过滤 7 日内到期资产 |
| 3 | `i64` | 过滤总余额资产 |

`m1` 由参数直接构造，因此查询通常会返回一行。用户没有匹配资产时，两个 LEFT JOIN 字段为 `NULL`；子查询里的 `IFNULL(sum(...), 0)` 不会为一个根本不存在的分组制造行。

## 返回

```rust
Result<Option<CoinsBalanceRow>, sqlx::Error>
```

```rust
pub struct CoinsBalanceRow {
    pub total_coins: Option<Decimal>,
    pub expire_coins_7d: Option<Decimal>,
}
```

- SQL 行存在时返回 `Some(CoinsBalanceRow)`。
- 无匹配资产时通常仍是 `Some`，但两个字段可以是 `None`。
- builtin 在构造展示数据时把空字段转换为 `0`。
- SQL 执行或字段解码失败时返回 `sqlx::Error`。

## 时间单位注意事项

当前 SQL 对同一列使用了两种尺度：

- `expire_coins_7d` 使用 `UNIX_TIMESTAMP() * 1000`，按毫秒比较。
- `total_coins` 使用 `UNIX_TIMESTAMP()`，按秒比较。

这是代码中的原样行为，不应在文档中统一解释为“全部是毫秒”。如果 `expire_time` 实际统一存储毫秒时间戳，`total_coins` 的过滤范围会与预期不同；修改前应先用真实数据确认字段单位。

两个子查询都使用 MySQL 服务器的当前时间，而不是应用传入时间。

## Redis 状态

`query_coins_balance_cached` 使用通用 cache-aside：Redis 失败会回退到外部数据库，成功查询后以 JSON 写入 60 秒 TTL。该包装目前没有生产调用方，所以：

- 余额变化不会受到这份 Redis 缓存影响。
- Redis 中即使存在 `balance:coins:{user_id}`，当前 builtin 也不会读取它。
- 当前没有余额缓存失效调用；如果未来启用包装，需要同时设计余额变更后的失效策略。
