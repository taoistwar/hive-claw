# 外部数据库

HiveClaw 连接一个只读的外部 MySQL 数据库（`ext_pool`），用于查询用户会员、游戏等业务数据。

> **注意：** 当前所有外部数据库操作均为 **只读（SELECT）**，无 INSERT/UPDATE/DELETE。

## 数据库连接

`ext_pool` 在 `AppState` 中作为 `Option<sqlx::MySqlPool>` 存在。当外部 DB 未配置时，相关功能不可用。

环境变量：

| 变量 | 说明 |
|------|------|
| `EXT_DATABASE_URL` | 外部 MySQL 连接字符串 |

## 外部表一览

| 表名 | 用途 | 查询文件 |
|------|------|----------|
| `cloud_user` | 云用户账号 | [membership](membership.md) |
| `cc_user_membership` | 会员等级与有效期 | [membership](membership.md) |
| `cc_user_asset_coin` | 金币资产与时长卡 | [membership](membership.md) |
| `cc_user_disk` | 网盘空间与到期时间 | [membership](membership.md) |
| `cc_user_subscription` | 订阅记录 | [membership](membership.md) |
| `cc_membership_level` | 会员等级定义 | [membership](membership.md) |
| `cc_logic_game` | 逻辑游戏主表 | [game](game.md) |
| `cc_logic_game_wide` | 游戏扩展信息 | [game](game.md) |
| `cc_game` | 游戏实例 | [game](game.md) |
| `cc_logic_game_version` | 游戏版本 | [game](game.md) |
| `cc_promotion_channel` | 推广渠道 | [game](game.md) |
| `cc_logic_game_exclude` | 游戏排除列表 | [game](game.md) |
| `cc_logic_game_blacklist` | 游戏黑名单 | [game](game.md) |

## 缓存策略

所有外部 DB 查询均支持 Redis 缓存（cache-aside 模式），详见 [cache_helper](../crates/hiveweb/src/services/cache_helper.rs)。

| 缓存键前缀 | TTL | 说明 |
|---|---|---|
| `cloud_user:exists:{id}` | 24h（存在）/ 5min（不存在） | 用户存在性 |
| `cloud_user:info:{id}` | 15min | 用户信息 |
| `vip:status:{id}` | 5min | VIP 状态 |
| `balance:{id}` | 1min | 余额 |
| `subscriptions:{id}` | 5min | 订阅列表 |
| `duration_cards:{id}` | 5min | 时长卡 |
| `game_info:{id}` | 15min | 游戏详情 |
| `game_list:{chan}:{type}` | 10min | 游戏列表 |
| `external_games:list` | 30min | 全部外部游戏 |
