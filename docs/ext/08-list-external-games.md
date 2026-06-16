# 08 · `list_external_games` — 按渠道 + 客户端类型列出可玩游戏

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::game_service::list_external_games` |
| 缓存包装 | `list_external_games_cached`（`cached_or_fetch`） |
| 缓存键 | `game_list:{channel}:{client_type}` |
| 缓存 TTL | 600 s（10 min） |
| 源文件 | [services/game_service.rs L382-415](../../crates/hiveweb/src/services/game_service.rs#L382-L415) |
| 调用方 | builtin [`game_list`](../../crates/hiveweb/src/runtime/builtins/game_list.rs)（Agent 列出可玩游戏）；[`api/game.rs`](../../crates/hiveweb/src/api/game.rs) `POST /game/list`（运营后台） |

## SQL 原文

```sql
SELECT DISTINCT ga.id, g.name, COALESCE(g.alias, '') AS alias
FROM cc_logic_game g
INNER JOIN cc_logic_game_wide       w  ON w.logic_game_id   = g.id
INNER JOIN cc_game                  ga ON ga.logic_game_id  = g.id
INNER JOIN cc_logic_game_version   v  ON w.version         = v.version
LEFT JOIN cc_logic_game_exclude     e
       ON w.logic_game_id = e.logic_game_id
      AND e.client_type   = w.client_type
      AND e.channel        = ?
LEFT JOIN cc_logic_game_blacklist   bl ON w.logic_game_id   = bl.logic_game_id
WHERE w.client_type  = ?
  AND w.channel_game_tag = COALESCE(
        (SELECT pc.game_tag FROM cc_promotion_channel pc WHERE pc.prom_channel = ? LIMIT 1),
        'UNKNOWN'
      )
  AND e.id  IS NULL
  AND bl.id IS NULL
ORDER BY ga.id
```

## 作用

在外部库中按 **渠道（channel）** 和 **客户端类型（client_type）** 筛选当前用户**可玩**的游戏。过滤链：

1. 用 `channel` 查 `cc_promotion_channel` 拿到 `game_tag`（找不到 → 用 `'UNKNOWN'` 兜底，保证查询不报错）
2. 在 `cc_logic_game_wide` 中匹配 `client_type` + `channel_game_tag`
3. 排除 `cc_logic_game_exclude` 中**针对该渠道 + 客户端类型**显式排除的游戏
4. 排除黑名单 `cc_logic_game_blacklist`
5. 返回去重后的 `(ga.id, g.name, alias)`

`DISTINCT` 是必需的——`cc_logic_game_wide` 同一 `logic_game_id` 可能对应多个 `version`，JOIN 后会产生重复。

## 参数

| 占位符 | 类型 | 出现 | 含义 |
| ------ | ---- | ---- | ---- |
| `?`    | `&str` | 1 | `cc_logic_game_exclude.channel` 渠道码 |
| `?`    | `&str` | 2 | `cc_logic_game_wide.client_type` 客户端类型 |
| `?`    | `&str` | 3 | `cc_promotion_channel.prom_channel` 渠道码（用于解析 game_tag） |

> 注意 `bind` 顺序：第 1、2 个 `?` 都是 `channel`（代码里 `.bind(channel).bind(client_type).bind(channel)`），第 2 个是 `client_type`。

## 返回

| Rust 类型 | 描述 |
| --------- | ---- |
| `Result<Vec<(u32, String, String)>, AppError>` | `(cc_game.id, cc_logic_game.name, alias)`，按 `ga.id` 升序 |

## 涉及的表 / 列

| 表 | 角色 | 关键列 |
| -- | ---- | ------ |
| `cc_logic_game` | 主表 | `id`, `name`, `alias` |
| `cc_logic_game_wide` | 过滤 + 客户端关联 | `logic_game_id`, `client_type`, `channel_game_tag`, `version` |
| `cc_logic_game_version` | version 校验 | `version` |
| `cc_game` | 上架条目 | `id`（返回）, `logic_game_id` |
| `cc_logic_game_exclude` | 渠道级排除 | `logic_game_id`, `client_type`, `channel` |
| `cc_logic_game_blacklist` | 全局黑名单 | `logic_game_id` |
| `cc_promotion_channel` | 渠道 → 标签 | `prom_channel`, `game_tag` |

## 失败 / 边界

- `cc_promotion_channel` 找不到 → `COALESCE(..., 'UNKNOWN')` 兜底，匹配不到任何游戏 → 返回空 `Vec`（**不报错**）。
- `cc_logic_game_version` 缺失 → 整条 SQL 失败。
- `e.id IS NULL` 模式：LEFT JOIN 找不到匹配 → 视为「未排除」。
