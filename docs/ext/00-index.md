# hiveweb 外部数据库 SQL 查询文档索引

> 本目录枚举 `crates/hiveweb` 下**所有直接对外部数据库（`EXTERNAL_DB_URL` → `cloud_computer`）执行 SQL 的查询**。
>
> 外部数据库在 `AppState` 中以 `ext_pool: Option<MySqlPool>` 形式注入，未配置时调用方需自行降级（多数调用方在缺池时返回 `"Assistant service unavailable"` 或 builtin 报错 `"外部数据库未配置"`）。
>
> 数据库驱动：MySQL（`sqlx::mysql`），连接池参数见 [src/db/connection.rs](../../crates/hiveweb/src/db/connection.rs)。
> 所有查询均使用 **预编译 + 参数绑定**（`sqlx::query_as` / `query_scalar` + `.bind(...)`），不存在 SQL 注入风险。
> 多数查询提供 Redis cache-aside 包装（`cached_or_fetch`），缓存键与 TTL 见 [cache_helper.rs](../../crates/hiveweb/src/services/cache_helper.rs)。

## 一览表

| # | 文档 | SQL 函数 | 目标表 | 缓存包装 | TTL |
| - | ---- | -------- | ------ | -------- | --- |
| 01 | [01-check-vip-membership.md](01-check-vip-membership.md) | `check_vip_membership` | `cc_user_membership` | `check_vip_membership_cached` | 300 s |
| 02 | [02-user-exists-in-cloud.md](02-user-exists-in-cloud.md) | `user_exists_in_cloud` | `cloud_user` | `user_exists_in_cloud_cached` | 存在 86400 s / 不存在 300 s |
| 03 | [03-get-cloud-user-info.md](03-get-cloud-user-info.md) | `get_cloud_user_info` | `cloud_user` | `get_cloud_user_info_cached` | 900 s |
| 04 | [04-query-membership-balance.md](04-query-membership-balance.md) | `query_membership_balance` | `cc_user_asset_coin` × `cc_user_disk` | `query_membership_balance_cached` | 60 s |
| 05 | [05-query-membership-subscriptions.md](05-query-membership-subscriptions.md) | `query_membership_subscriptions` | `cc_user_membership` + `cc_membership_level` + `cc_user_subscription` | `query_membership_subscriptions_cached` | 300 s |
| 06 | [06-query-duration-cards.md](06-query-duration-cards.md) | `query_duration_cards` | `cc_user_asset_coin` + `cc_order` | `query_duration_cards_cached` | 300 s |
| 07 | [07-get-external-game-by-id.md](07-get-external-game-by-id.md) | `get_external_game_by_id` | `cc_logic_game_wide` + `cc_game` + `cc_game_platform` + `cc_logic_game_version` + `cc_promotion_channel` + `cc_logic_game_exclude` + `cc_logic_game_blacklist` | `get_external_game_by_id_cached` | 900 s |
| 08 | [08-list-external-games.md](08-list-external-games.md) | `list_external_games` | `cc_logic_game_wide` + `cc_logic_game_exclude` + `cc_logic_game_version` + `cc_logic_game_blacklist` + `cc_logic_game` | `list_external_games_cached` | 600 s |
| 09 | [09-get-game-channels.md](09-get-game-channels.md) | `get_game_channels` | `cc_logic_game_wide` + `cc_promotion_channel` + `cc_logic_game_exclude` | — | — |
| 10 | [10-get-game-client-types.md](10-get-game-client-types.md) | `get_game_client_types` | `cc_logic_game_wide` + `cc_logic_game_exclude` + `cc_logic_game_version` + `cc_logic_game_blacklist` | — | — |
| 11 | [11-get-trial-purchase-platform-config.md](11-get-trial-purchase-platform-config.md) | `get_trial_purchase_platform_config` | `cc_config` | — | — |

## 源代码位置

所有外部 SQL 集中在两个文件：

- [`crates/hiveweb/src/services/membership.rs`](../../crates/hiveweb/src/services/membership.rs) — 会员 / 用户 / 时长卡 6 条
- [`crates/hiveweb/src/services/game_service.rs`](../../crates/hiveweb/src/services/game_service.rs) — 游戏 / 渠道 / 客户端类型 / 试玩配置 5 条

## 池注入路径

```
.env  EXTERNAL_DB_URL  ──►  main.rs  ──►  create_router(... ext_pool ...)
                                  │
                                  └─►  AppState { ext_pool: Option<MySqlPool> }
                                          │
                                          ├──► api/* (chat_assistant, newsession, game, workflow, runtime)
                                          └──► runtime/builtins/* (query_balance, game_info, game_list)
```

## 公共行为约定

1. **错误处理**：所有 SQL 错误一律用 `AppError::Internal("…query: {e}")` 包装后向上抛；缓存层用 `String` 错误向前透传。
2. **时区 / 时间**：`effective_end_time`、`start_time`、`end_time`、`next_billing_time` 等业务时间字段是 `NaiveDateTime`（UTC 存储）。
3. **UNIX 时间戳（毫秒）**：`cc_user_asset_coin.expire_time` 与 `cc_user_disk.end_time/start_time` 存的是 `BIGINT` 毫秒戳，比较时使用 `UNIX_TIMESTAMP() * 1000`。
4. **可空列**：所有 `Option<…>` 列都允许 NULL；调用方需按业务场景判断空值。
5. **不修改外部库**：本服务对外部库只做 SELECT，不写不更新。
6. **DB 侧时间过滤（v2 模式）**：`check_vip_membership` 与 `query_membership_subscriptions` 已改为在 SQL 内用 `now()` 同时判断 `effective_start_time < now()` 与 `effective_end_time > now()`，把"当前有效"这一判断推给 DB，减少无效行回传。
