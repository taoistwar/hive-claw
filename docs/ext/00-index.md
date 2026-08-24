# hiveweb 外部数据库查询文档索引

本目录记录 hiveweb 通过 `EXTERNAL_DB_URL` 访问外部 MySQL 数据库的主要业务查询及其实际调用方式。

> 本索引是人工维护的业务查询清单，**不是代码中所有外部 SQL 的穷举**。除下表外，`api/game.rs`、`runtime/builtins/game_info.rs`、`services/membership.rs` 和 `services/game_service.rs` 还包含配置、标签解析、管理端详情等辅助查询。审计完整调用面时应同时搜索 `ext_pool`、`EXTERNAL_DB_URL` 和 `sqlx::query*`。

外部连接池以 `ext_pool: Option<MySqlPool>` 注入 `AppState`。未配置外部数据库时，由具体 HTTP 接口或 builtin 按各自语义返回错误，不能统一假设为某一条固定错误消息。

## 废弃功能边界

管理中心的“推荐游戏”和“游戏别名管理”前后端业务已经废弃。仓库中
`recommended_games*`、`/api/recommended-games*`、`/api/game-aliases*` 及其管理页面
仅为兼容历史数据或调用保留；新代码不得新增依赖、调用或功能扩展。

这不影响 Agent builtin 的按分类游戏查询/推荐，也不影响外部数据库中的
`cc_logic_game.alias` 或 `cc_ranking_recommended_game.recommend_reason`。后两者属于外部
游戏数据，不等同于已废弃的本地游戏别名管理和 `recommended_games` 业务。

## 已维护查询

| # | 文档 | 当前 SQL 函数 | 目标表 | Redis 当前实际状态 |
| - | ---- | ------------ | ------ | ------------------ |
| 01 | [VIP 会员资格校验](01-check-vip-membership.md) | `check_vip_membership` | `cc_user_membership` | 不使用；旧 VIP 缓存包装已删除 |
| 02 | [金币余额](02-query-coins-balance.md) | `query_coins_balance` | `cc_user_asset_coin` | 包装已定义，但当前调用方直接查 DB |
| 03 | [云电脑用户身份](03-get-cloud-user-info.md) | `get_cloud_user_info` | `cloud_user` | **已使用** `get_cloud_user_info_cached`，900 秒 |
| 04 | [云硬盘余额](04-query-disk-balance.md) | `query_disk_balance` | `cc_user_disk` | 包装已定义，但当前调用方直接查 DB |
| 05 | [会员与订阅](05-query-membership-subscriptions.md) | `query_membership_subscriptions` | 会员、等级、订阅、订单相关表 | 包装已定义，但当前调用方直接查 DB |
| 06 | [时长卡](06-query-duration-cards.md) | `query_duration_cards` | 用户资产、订单、产品相关表 | 包装已定义，但当前调用方直接查 DB |
| 07 | [按 ID 查询外部游戏](07-get-external-game-by-id.md) | `get_external_game_by_id` | 游戏、平台、渠道及上下架相关表 | 当前函数直接查 DB |
| 08 | [外部游戏列表](08-list-external-games.md) | `list_external_games` | 游戏、版本及上下架相关表 | 包装已定义，但主要 builtin 当前直接查 DB |
| 09 | [游戏渠道](09-get-game-channels.md) | `get_game_channels` | 游戏宽表、推广渠道、排除表 | 不使用 |
| 10 | [游戏客户端类型](10-get-game-client-types.md) | `get_game_client_types` | 游戏宽表、渠道、版本及上下架相关表 | 不使用 |
| 11 | [试玩购买平台配置](11-get-trial-purchase-platform-config.md) | `get_trial_purchase_platform_config` | `cc_config` | 不使用 |

已删除的旧接口不再单独保留为当前查询文档：

- `user_exists_in_cloud` 已合并到 03；`get_cloud_user_info` 的 `Some` / `None` 同时表达用户是否存在。
- `query_membership_balance` 已拆分为 02 的 `query_coins_balance` 和 04 的 `query_disk_balance`。

## 辅助查询（未单独成篇）

下列查询也会访问 `EXTERNAL_DB_URL`，但目前没有各自的详情页。它们列在这里，避免排查外部数据库负载、缓存或数据权限时遗漏：

| 函数 / 位置 | 目的与实际调用 | 外部表 | Redis 当前实际状态 |
| ----------- | ------------ | ------ | ------------------ |
| `membership::get_ai_assistant_chat_limit_config` | Assistant 对话、quota 和 Legacy 推荐游戏执行接口读取每日次数、提醒阈值与重置小时 | `cc_config`，标签 `AIassistantChatLimitConfig`、状态 `ACTIVE` | 每次直接查 DB；同名缓存键和 TTL 常量当前未使用 |
| `membership::get_discounted_products_config` | `query_balance(category=discount)` 按客户端和渠道生成优惠卡片 | `cc_config`，标签 `AIDiscountedProducts`、状态 `ACTIVE` | 不使用；失败降级为“暂无产品优惠活动” |
| `game_info::fetch_categories` | `game_info` 无法取得有效游戏 ID 时，读取类型 1 的分类供 LLM 选择 | `cc_game_tag` | **已使用** `game_tags:cc_game_tag_type1`，600 秒；未注入 Redis 时直接查 DB |
| `game_service::fetch_logic_game_ids_by_tag` | 按分类、客户端和渠道筛选版本有效、未排除、未拉黑且已上架的候选游戏 | 游戏宽表、版本、排除、黑名单及游戏主表 | 不使用 |
| `game_service::filter_available_games` | 刷新历史游戏卡片前，批量确认游戏是否仍为 `status = 1` | `cc_logic_game` | 不使用；另有单次请求内去重，不是 Redis 缓存 |
| `api/game.rs::get_external_games` 内联 SQL | 管理端外部游戏选择列表，只读取 ID 和名称 | `cc_logic_game` | **已使用** `external_games:list`，1800 秒 |
| `game_service::get_external_game_detail` | 管理端外部游戏基础详情；随后复用 09、10 查询渠道和客户端类型 | `cc_logic_game`、`cc_logic_game_wide` | 不使用 |
| `recommended_game::fetch_top_filtered` / `fetch_by_tag`（Legacy） | 已废弃的公开推荐列表按运营标签、客户端和渠道随机筛选候选；仅兼容保留 | `recommended_games` 及游戏相关外部表 | 不使用 |
| `recommended_game::fetch_by_game_id`（Legacy） | 已废弃的推荐游戏执行接口按 `game_id` 读取推荐内容；仅兼容保留 | `recommended_games` | 不使用 |

Legacy `recommended_games` 代码存在双库用法：管理端 CRUD / 列表把 hiveweb 主库连接池传给 service，而公开推荐列表和执行接口把 `ext_pool` 传入同一 service。排查遗留流量时，必须从调用点追踪实际传入的连接池，不能只看形参名 `pool`；新代码不得沿用该双库模式。

## 主要源代码位置

- [`services/membership.rs`](../../crates/hiveweb/src/services/membership.rs)：会员、用户、金币、云硬盘、时长卡和外部配置查询。
- [`services/game_service.rs`](../../crates/hiveweb/src/services/game_service.rs)：游戏、渠道、客户端类型和试玩配置查询。
- [`api/game.rs`](../../crates/hiveweb/src/api/game.rs)：管理端外部游戏列表和详情查询。
- [`services/recommended_game.rs`](../../crates/hiveweb/src/services/recommended_game.rs)：Legacy 推荐游戏查询，仅供排查兼容代码；调用方决定它使用主库还是外部库。
- [`api/chat_messages.rs`](../../crates/hiveweb/src/api/chat_messages.rs)：刷新历史游戏卡片，并在单次请求内复用查询结果。
- [`runtime/builtins`](../../crates/hiveweb/src/runtime/builtins)：部分 Agent builtin 直接执行或触发外部查询。
- [`services/cache_helper.rs`](../../crates/hiveweb/src/services/cache_helper.rs)：通用 Redis cache-aside、缓存键和 TTL。

## 连接池注入路径

```text
EXTERNAL_DB_URL
    └─> main.rs 创建 Option<MySqlPool>
          └─> AppState.ext_pool
                ├─> api/*
                └─> runtime / builtins / services
```

连接池参数见 [`db/connection.rs`](../../crates/hiveweb/src/db/connection.rs)。

## 阅读约定

### 参数与 SQL

01–04 当前都通过 SQLx 的 `.bind(...)` 绑定外部输入。其他辅助查询中存在动态构造 SQL 的场景，应逐条审计，不能把本结论扩展为“仓库内所有 SQL 都是静态预编译 SQL”。例如 `fetch_logic_game_ids_by_tag` 会把从 `cc_game_tag` 取得、再经 LLM 选择的 `category_name` 内插到 `JSON_CONTAINS` SQL，其余条件才使用 `.bind(...)`。

### Redis

缓存包装函数“存在”不等于生产调用链“已启用”。本目录分别标注：

- **已使用**：调用方实际执行 cache-aside。
- **包装已定义但未调用**：键和 TTL 只代表备用实现，当前结果仍来自外部数据库。
- **不使用**：没有对应缓存路径。

通用 `cached_or_fetch` 的 Redis GET / SETEX 是 best-effort：Redis 失败时回源数据库，数据库查询失败才向上传递错误。当前没有为这些外部数据实现统一的主动失效机制。

### 时间

不同查询的时间来源和单位并不统一：

- VIP 查询由 Rust 生成 UTC+8 `NaiveDateTime` 并作为参数绑定，不使用数据库 `NOW()`。
- 金币和云硬盘查询使用 MySQL `UNIX_TIMESTAMP()`；金币总额与 7 日到期子查询目前甚至使用不同的秒 / 毫秒尺度。
- `DATETIME`、秒时间戳和毫秒时间戳不能互换，具体以各查询文档和 SQL 原文为准。

### 错误与空值

- DB-only 函数可能返回 `sqlx::Error`，也可能在服务层转换为 `AppError` 或 `String`；调用方的降级策略各不相同。
- `Option<Row>` 的 `None` 通常表示没有匹配行；`Row` 内的 `Option<T>` 表示 LEFT JOIN、聚合或可空列没有值。
- SQLx 映射到非 `Option` 字段时，数据库 `NULL` 会成为解码错误，而不是自动变成空字符串或零。

01–04 所记录的查询均为 `SELECT`，不会修改外部数据库。
