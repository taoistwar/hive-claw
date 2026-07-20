# 07 · `get_external_game_by_id` — 按游戏、客户端和渠道查询可用实例

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::game_service::get_external_game_by_id` |
| 返回类型 | `Result<Vec<ExternalGameInfo>, AppError>` |
| 源文件 | [`services/game_service.rs`](../../crates/hiveweb/src/services/game_service.rs) |
| 生产调用 | `get_single_external_game_info`（供 builtin [`game_info`](../../crates/hiveweb/src/runtime/builtins/game_info.rs) 和 Legacy 推荐游戏兼容接口调用）；聊天消息卡片补全会直接调用本函数 |
| 相关缓存包装 | `get_single_external_game_info_cached`，已定义但当前生产代码**没有调用** |
| 缓存键 / TTL | 包装函数被调用时使用 `game_info:single:{game_id}:{client_type}:{channel}`；查到为 86400 秒，未查到为 300 秒 |

不存在名为 `get_external_game_by_id_cached` 的函数。缓存包装针对的是“排序后取一条”的 `Option<ExternalGameInfo>`，不是本函数返回的完整 `Vec`。

## 返回结构 `ExternalGameInfo`

```rust
pub struct ExternalGameInfo {
    pub logic_game_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub cover_image: Option<String>,
    pub game_tags: Option<serde_json::Value>,
    pub computer_id: Option<i64>,
    pub platform_name: Option<String>,
    pub client_type: Option<String>,
    pub channel: Option<String>,
    pub game_icon: Option<String>,
    pub raw_description: Option<String>,
}
```

这里的 `description` 是外部表 `cc_ranking_recommended_game` 最新记录的
`recommend_reason`；游戏宽表自身的描述放在 `raw_description`。该外部推荐理由仍是
游戏详情数据，与已废弃的本地 `recommended_games` 业务不是同一功能。

## SQL 原文

```sql
SELECT
  t1.logic_game_id,
  t1.name,
  t9.recommend_reason AS description,
  t1.description AS raw_description,
  t1.cover_image,
  t1.game_tags,
  t2.computer_id,
  t3.name AS platform_name,
  t1.client_type,
  t5.prom_channel AS channel,
  t8.game_icon
FROM (
  SELECT * FROM cc_logic_game_wide
  WHERE logic_game_id = ? AND lower(client_type) = lower(?)
) t1
INNER JOIN (
  SELECT * FROM cc_game WHERE logic_game_id = ?
) t2 ON t1.logic_game_id = t2.logic_game_id
INNER JOIN cc_game_platform t3
  ON t2.game_platform_id = t3.id
INNER JOIN (
  SELECT * FROM cc_computer_info WHERE status = 1
) ci ON t2.computer_id = ci.id
INNER JOIN cc_logic_game_version t4
  ON t1.version = t4.version
INNER JOIN (
  SELECT pc.id, pc.game_tag, pc.prom_channel
  FROM cc_promotion_channel pc
  WHERE pc.prom_channel = ?
) t5 ON t1.channel_game_tag = t5.game_tag
LEFT JOIN (
  SELECT * FROM cc_logic_game_exclude
  WHERE client_type = ? AND channel = ?
) t6 ON t1.logic_game_id = t6.logic_game_id
LEFT JOIN cc_logic_game_blacklist t7
  ON t1.logic_game_id = t7.logic_game_id
LEFT JOIN (
  SELECT * FROM cc_logic_game WHERE id = ? AND status = 1
) t8 ON t1.logic_game_id = t8.id
LEFT JOIN (
  SELECT * FROM cc_ranking_recommended_game
  WHERE logic_game_id = ?
  ORDER BY update_time DESC
  LIMIT 1
) t9 ON t1.logic_game_id = t9.logic_game_id
WHERE t6.id IS NULL
  AND t7.id IS NULL
```

## 作用与过滤链

本函数按 `logic_game_id + client_type + channel` 查询外部游戏实例。只有同时满足以下条件的行才能返回：

1. `cc_logic_game_wide` 存在指定游戏和客户端；客户端使用 `lower(...)` 做大小写不敏感比较；
2. 存在对应 `cc_game`；
3. `game_platform_id` 能关联平台；
4. `computer_id` 能关联一台 `status = 1` 的电脑；
5. 宽表版本存在于 `cc_logic_game_version`；
6. `channel_game_tag` 能映射到指定 `prom_channel`；
7. 没有命中同客户端、同渠道的排除表；
8. 没有命中全局黑名单。

`cc_logic_game` 的 `status = 1` 检查位于 `LEFT JOIN t8` 内，只影响 `game_icon` 是否能取到，**不会淘汰整行**。`cc_promotion_channel` 当前只过滤 `prom_channel`，没有额外检查其 `status`。

## 参数与 bind 顺序

函数签名为：

```rust
get_external_game_by_id(ext_pool, logic_game_id, client_type, channel)
```

SQL 共 8 个 bind：

| 次序 | 值 | 过滤位置 |
| ---- | -- | -------- |
| 1 | `logic_game_id` | `cc_logic_game_wide.logic_game_id` |
| 2 | `client_type` | `lower(cc_logic_game_wide.client_type)` |
| 3 | `logic_game_id` | `cc_game.logic_game_id` |
| 4 | `channel` | `cc_promotion_channel.prom_channel` |
| 5 | `client_type` | `cc_logic_game_exclude.client_type` |
| 6 | `channel` | `cc_logic_game_exclude.channel` |
| 7 | `logic_game_id` | 活跃 `cc_logic_game.id`（用于取图标） |
| 8 | `logic_game_id` | `cc_ranking_recommended_game.logic_game_id` |

## 多行与“取一条”规则

本函数使用 `fetch_all`，没有 `ORDER BY`，可因多条宽表行或多个 `cc_game` 实例返回 0..N 行。

生产调用通常通过非缓存的 `get_single_external_game_info`：

1. 调用本函数取得 `Vec`；
2. 读取 `cc_config.label = 'trialPurchasePlatformConfig'` 的 `platformPriority`；
3. 按平台优先级稳定排序；
4. 返回第一条 `Option<ExternalGameInfo>`。

配置缺失或读取失败时不排序，此时选中的第一条取决于数据库未定义的返回顺序。

## 空结果与错误

- 游戏、客户端、渠道映射、平台、活跃电脑或版本任一必需关联缺失：返回空 `Vec`。
- 命中渠道排除或全局黑名单：返回空 `Vec`。
- 没有推荐记录：仍返回游戏，`description = NULL`。
- 活跃 `cc_logic_game` 未匹配：仍返回游戏，`game_icon = NULL`。
- `get_single_external_game_info` 将空 `Vec` 转为 `None`；`game_info` builtin 对直接查询输出 `found: false` 和“未找到相关游戏”。
- SQL 失败：包装为 `AppError::Internal("game_info external query: ...")`。

## 缓存现状

`get_single_external_game_info_cached` 实现了 Redis cache-aside，并缓存 `Some` 和 `None`：

- 查到游戏：缓存 1 天；
- 未查到游戏：缓存 5 分钟；
- Redis 读写失败：降级查询外部数据库；
- 缓存 miss 时仍会执行平台优先级排序。

但当前 builtin、Legacy 推荐兼容接口和聊天消息处理均调用非缓存版本或直接调用本函数，因此这套 Redis 缓存目前没有生产消费者。
