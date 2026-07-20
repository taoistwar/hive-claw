# 04 · `query_disk_balance` — 云硬盘配额与到期时间

## 元数据

| 字段 | 当前实现 |
| ---- | -------- |
| Rust 函数 | `services::membership::query_disk_balance` |
| 源文件 | [`services/membership.rs`](../../crates/hiveweb/src/services/membership.rs) |
| 外部表 | `cc_user_disk` |
| 当前调用方 | builtin [`query_balance`](../../crates/hiveweb/src/runtime/builtins/query_balance.rs)，`category=disk` 或 `category=benefits` |
| 当前调用方式 | 直接查询外部数据库 |
| Redis 包装 | `query_disk_balance_cached` 已定义，但当前生产调用链**没有调用它** |
| 未使用缓存键 / TTL | `balance:disk:{user_id}` / 60 秒 |

原来的 `query_membership_balance` 合并查询已不存在。金币和云硬盘现在分别由 `query_coins_balance` 与 `query_disk_balance` 查询。

## SQL 原文

```sql
SELECT user_id, sum(size/1024/1024/1024) as disk_total_size,
       MAX(end_time) as disk_end_time, status as disk_status
from cc_user_disk
where user_id=?
  and end_time > UNIX_TIMESTAMP()*1000
  and start_time < UNIX_TIMESTAMP()*1000
  AND status != 'EXPIRED'
group by user_id
```

## 统计口径

查询只聚合满足以下全部条件的云硬盘购买记录：

- `user_id` 与输入用户一致。
- `end_time` 严格晚于 MySQL 当前时间。
- `start_time` 严格早于 MySQL 当前时间。
- `status` 不等于 `EXPIRED`。

返回指标：

| 字段 | Rust 类型 | 含义 |
| ---- | --------- | ---- |
| `disk_total_size` | `Option<Decimal>` | 匹配记录的 `size` 合计，连续除以 1024 三次后按 GB 返回 |
| `disk_end_time` | `Option<i64>` | 匹配记录中最大的 `end_time`，即最晚到期时间 |
| `disk_status` | `Option<String>` | 分组中某条匹配记录的状态，见下方确定性说明 |

SQL 还选择了 `user_id`，但 `DiskBalanceRow` 没有该字段，SQLx 映射时不会把它暴露给调用方。

## 参数

| 顺序 | Rust 类型 | 含义 |
| ---- | --------- | ---- |
| 1 | `i64` | `cc_user_disk.user_id` |

## 返回

```rust
Result<Option<DiskBalanceRow>, sqlx::Error>
```

```rust
pub struct DiskBalanceRow {
    pub disk_total_size: Option<Decimal>,
    pub disk_end_time: Option<i64>,
    pub disk_status: Option<String>,
}
```

- 至少有一条匹配记录时返回聚合后的 `Some(DiskBalanceRow)`。
- 用户没有云硬盘，或所有记录都未生效、已到期、状态为 `EXPIRED` / `NULL` 时返回 `None`。
- SQL 执行、严格分组校验或字段解码失败时返回 `sqlx::Error`。

## 时间约定

`start_time` 和 `end_time` 按 SQL 中的写法与 `UNIX_TIMESTAMP() * 1000` 比较，即查询把它们视为毫秒 UNIX 时间戳。当前时间来自 MySQL 服务器：

- `start_time == 当前毫秒` 的记录尚不命中。
- `end_time == 当前毫秒` 的记录已经不命中。

无参数的 `UNIX_TIMESTAMP()` 取数据库服务器当前时刻对应的 epoch 秒；这条查询没有复用 VIP 查询中由 Rust 绑定的 UTC+8 `current_time`。比较是否正确主要取决于服务器时钟以及 `start_time` / `end_time` 是否确实存为毫秒 epoch。

## `disk_status` 的确定性

SQL 按 `user_id` 分组，却直接选择了既未聚合、也未加入 `GROUP BY` 的 `status`：

```sql
status as disk_status
```

因此需要注意：

- 开启 MySQL `ONLY_FULL_GROUP_BY` 时，多条记录具有不同状态可能导致查询报错。
- 未开启严格分组时，MySQL 可从分组中选择任意一条记录的 `status`，结果不保证与 `MAX(end_time)` 所在记录一致。

如果业务要求展示“最晚到期那条记录的状态”，应修改 SQL 明确关联到最大 `end_time` 的记录；当前文档只描述现状。

## Redis 状态

`query_disk_balance_cached` 使用通用 cache-aside，键为 `balance:disk:{user_id}`，TTL 为 60 秒。该包装目前没有生产调用方，builtin 直接查询外部数据库。因此 Redis 中现有同名键不会影响当前查询结果，也没有实际生效的主动失效流程。
