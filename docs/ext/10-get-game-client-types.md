# 10 · `get_game_client_types` — 查询游戏的可用客户端类型

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::game_service::get_game_client_types` |
| 缓存包装 | **无** |
| 源文件 | [services/game_service.rs](../../crates/hiveweb/src/services/game_service.rs) |
| 直接调用方 | [`api/game.rs`](../../crates/hiveweb/src/api/game.rs)；Legacy [`api/recommended_game.rs`](../../crates/hiveweb/src/api/recommended_game.rs) |

## SQL 原文

```sql
SELECT t1.client_type
FROM (
    SELECT in1.*,in2.prom_channel
    FROM (
        select * from cc_logic_game_wide where logic_game_id = ?
    ) in1
    INNER JOIN cc_promotion_channel in2
        ON in2.game_tag = in1.channel_game_tag
       AND in2.status = 1
) t1
LEFT JOIN (
  select * from cc_logic_game_exclude where logic_game_id = ?
) t2
    ON t1.logic_game_id = t2.logic_game_id
   AND t1.prom_channel = t2.channel
INNER JOIN cc_logic_game_version t3 ON t1.version = t3.version
LEFT JOIN cc_logic_game_blacklist t4 ON t1.logic_game_id = t4.logic_game_id
where t2.id is null AND t4.id is null
group by t1.client_type
```

## 作用

根据 `logic_game_id` 返回仍有可用渠道的客户端类型：

1. 从 `cc_logic_game_wide` 读取游戏记录。
2. 按 `channel_game_tag` 关联 `cc_promotion_channel`，只保留 `status = 1` 的渠道，
   并把 `prom_channel` 带入外层查询。
3. 按“游戏 ID + 渠道码”排除 `cc_logic_game_exclude` 中的渠道。
4. 通过 `cc_logic_game_version.version` 校验版本存在，并排除
   `cc_logic_game_blacklist` 中的游戏。
5. 按 `client_type` 分组去重。

同一客户端类型只要还有一个启用且未被排除的渠道，就会保留在结果中。

## 参数

| 占位符 | Rust 类型 | 出现位置 | 含义 |
| ------ | --------- | -------- | ---- |
| 第 1 个 `?` | `i64` | `cc_logic_game_wide` 子查询 | 目标 `logic_game_id` |
| 第 2 个 `?` | `i64` | `cc_logic_game_exclude` 子查询 | 同一个 `logic_game_id` |

两个占位符均绑定传入的 `logic_game_id`。

## 配置与状态条件

| 表 | 条件 | 含义 |
| -- | ---- | ---- |
| `cc_promotion_channel` | `status = 1` | 客户端类型必须至少关联一个启用渠道 |
| `cc_logic_game_exclude` | `logic_game_id` 和 `channel` 同时匹配 | 排除对应渠道 |
| `cc_logic_game_version` | `t1.version = t3.version` | 只保留可关联到版本表的记录 |
| `cc_logic_game_blacklist` | `t4.id IS NULL` | 黑名单中的游戏不返回任何客户端类型 |

排除子查询没有按 `client_type` 过滤；它先按游戏筛选，再依据推广渠道与
`cc_logic_game_exclude.channel` 匹配。

## 返回

| Rust 类型 | 描述 |
| --------- | ---- |
| `Result<Vec<String>, AppError>` | 去重后的 `client_type` 字符串数组；没有可用类型时返回空数组 |

查询错误包装为 `AppError::Internal("game_client_types query: …")`。

## 实际调用

- 管理端 `GET /api/external-games/:id`：与渠道并行查询，写入
  `ExternalGameDetail.client_types`。调用方使用 `unwrap_or_default()`，查询失败时该字段
  返回空数组。
- Legacy 管理端 `POST /api/recommended-games` 或 `PUT /api/recommended-games/:id`：若策略的
  `client_type` 数组含有 `"*"`，
  `expand_strategy_wildcards` 会用本函数结果替换整个数组。查询失败时替换为空数组；
  游戏 ID 无法解析或未配置外部数据库时不会执行查询，并保留原数组。该路径仅兼容
  保留，新代码不得调用或依赖。

本函数没有独立 HTTP 接口，也没有 Redis 缓存。

## 涉及的表 / 列

| 表 | 角色 | 关键列 |
| -- | ---- | ------ |
| `cc_logic_game_wide` | 游戏和客户端类型来源 | `logic_game_id`、`channel_game_tag`、`version`、`client_type` |
| `cc_promotion_channel` | 启用渠道映射 | `game_tag`、`status`、`prom_channel` |
| `cc_logic_game_exclude` | 渠道排除 | `logic_game_id`、`channel`、`id` |
| `cc_logic_game_version` | 版本有效性关联 | `version` |
| `cc_logic_game_blacklist` | 游戏黑名单 | `logic_game_id`、`id` |

## 失败 / 边界

- 找不到游戏、没有启用渠道、版本无法关联、全部渠道被排除或游戏在黑名单中时，
  返回 `Vec::new()`。
- SQL 没有 `ORDER BY`；`GROUP BY` 只用于去重，不保证返回顺序。
- 返回项映射为非空 `String`；若数据库实际返回 `NULL`，SQLx 解码会报错。
