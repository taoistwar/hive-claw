# 09 · `get_game_channels` — 查询游戏的可用渠道

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::game_service::get_game_channels` |
| 缓存包装 | **无** |
| 源文件 | [services/game_service.rs](../../crates/hiveweb/src/services/game_service.rs) |
| 直接调用方 | [`api/game.rs`](../../crates/hiveweb/src/api/game.rs)；Legacy [`api/recommended_game.rs`](../../crates/hiveweb/src/api/recommended_game.rs) |

## SQL 原文

```sql
SELECT distinct t2.prom_channel
FROM (
    select * from cc_logic_game_wide where logic_game_id=?
) t1
INNER JOIN cc_promotion_channel t2
    ON t2.game_tag = t1.channel_game_tag
   AND t2.status = 1
LEFT JOIN (
    select * from cc_logic_game_exclude
) t3
    ON t1.logic_game_id = t3.logic_game_id
   AND t2.prom_channel = t3.channel
where t3.id is null
```

## 作用

根据 `logic_game_id` 返回该游戏当前可用的渠道码：

1. 从 `cc_logic_game_wide` 找出游戏的 `channel_game_tag`。
2. 通过 `cc_promotion_channel.game_tag` 映射渠道，并且只保留
   `status = 1` 的启用渠道。
3. 按“游戏 ID + 渠道码”关联 `cc_logic_game_exclude`；命中排除记录的渠道不返回。
4. 使用 `DISTINCT` 去重。

排除条件包含 `t2.prom_channel = t3.channel`，因此某个渠道被排除时，不会连带
排除同一游戏的其他渠道。

## 参数

| 占位符 | Rust 类型 | 含义 |
| ------ | --------- | ---- |
| `?` | `i64` | `cc_logic_game_wide.logic_game_id` |

## 配置与状态条件

| 表 | 条件 | 含义 |
| -- | ---- | ---- |
| `cc_promotion_channel` | `status = 1` | 只返回启用的推广渠道 |
| `cc_logic_game_exclude` | `logic_game_id` 和 `channel` 同时匹配 | 排除指定游戏的指定渠道 |

`cc_logic_game_exclude` 没有按 `client_type` 过滤。因此，只要存在相同游戏和渠道的
排除记录，该渠道就会从结果中移除，不区分客户端类型。

## 返回

| Rust 类型 | 描述 |
| --------- | ---- |
| `Result<Vec<String>, AppError>` | 去重后的 `prom_channel` 数组；没有可用渠道时返回空数组 |

查询错误包装为 `AppError::Internal("game_channels query: …")`。

## 实际调用

- 管理端 `GET /api/external-games/:id`：与客户端类型并行查询，写入
  `ExternalGameDetail.channels`。调用方使用 `unwrap_or_default()`，查询失败时该字段
  返回空数组。
- Legacy 管理端 `POST /api/recommended-games` 或 `PUT /api/recommended-games/:id`：若策略的
  `channel` 数组含有 `"*"`，
  `expand_strategy_wildcards` 会用本函数结果替换整个数组。查询失败时替换为空数组；
  游戏 ID 无法解析或未配置外部数据库时不会执行查询，并保留原数组。该路径仅兼容
  保留，新代码不得调用或依赖。

本函数没有独立 HTTP 接口，也没有 Redis 缓存。

## 涉及的表 / 列

| 表 | 关键列 |
| -- | ------ |
| `cc_logic_game_wide` | `logic_game_id`（过滤）/ `channel_game_tag`（JOIN 键） |
| `cc_promotion_channel` | `game_tag`（JOIN 键）/ `status`（启用过滤）/ `prom_channel`（返回和排除匹配） |
| `cc_logic_game_exclude` | `logic_game_id`、`channel`（排除匹配）/ `id`（判断是否命中） |

## 失败 / 边界

- 找不到游戏、没有启用渠道或所有渠道均被排除时，返回 `Vec::new()`。
- SQL 没有 `ORDER BY`，调用方不能依赖渠道顺序。
- 返回项映射为非空 `String`；若数据库实际返回 `NULL`，SQLx 解码会报错，而不是
  在数组中返回空值。
