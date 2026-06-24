# 08 · `list_external_games` — 按 client_type + channel 列出可玩游戏

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::game_service::list_external_games` |
| 缓存包装 | `list_external_games_cached`（`cached_or_fetch`） |
| 缓存键 | `game_list:{channel}:{client_type}` |
| 缓存 TTL | 600 s（10 min） |
| 源文件 | [services/game_service.rs L423-452](../../crates/hiveweb/src/services/game_service.rs#L423-L452) |
| 调用方 | builtin [`game_list`](../../crates/hiveweb/src/runtime/builtins/game_list.rs)（Agent 列出可玩游戏）；[`api/game.rs`](../../crates/hiveweb/src/api/game.rs) `POST /game/list`（运营后台） |

## SQL 原文

```sql
SELECT
  z2.id, z2.name, COALESCE(z2.alias, '') AS alias
FROM (
  SELECT t1.logic_game_id, t1.name
  FROM (
    SELECT * FROM cc_logic_game_wide WHERE client_type = ?
  ) t1
  LEFT JOIN (
    SELECT * FROM cc_logic_game_exclude
    WHERE client_type = ? AND channel = ?
  ) t2 ON t1.logic_game_id = t2.logic_game_id
  INNER JOIN cc_logic_game_version t3 ON t1.version = t3.version
  LEFT JOIN cc_logic_game_blacklist t4 ON t1.logic_game_id = t4.logic_game_id
  WHERE t2.id IS NULL AND t4.id IS NULL
  GROUP BY t1.logic_game_id, t1.name
) z1
INNER JOIN cc_logic_game z2 ON z1.logic_game_id = z2.id
```

## ⚠️ 最近变更

| 维度 | 旧 | 新 |
| ---- | -- | --- |
| 主入口 | `FROM cc_logic_game g INNER JOIN cc_logic_game_wide w ...` | `FROM (子查询 z1) INNER JOIN cc_logic_game z2 ...` |
| `client_type` 过滤 | 在 `w.client_type = ?` 主 WHERE | 在 `cc_logic_game_wide` 子查询内 `WHERE client_type = ?` |
| `channel_game_tag` 映射 | 用 `cc_promotion_channel` 子查询解析 `channel → game_tag` | **不再解析**（直接按 `client_type` 过滤，不再绑定到特定 channel 的 tag） |
| `channel` 过滤 | `e.channel = ?`（exclude 表）+ `COALESCE(... 'UNKNOWN')` | `e.channel = ?`（exclude 表） |
| 排除逻辑 | LEFT JOIN exclude + LEFT JOIN blacklist + WHERE id IS NULL | LEFT JOIN exclude + INNER JOIN version + LEFT JOIN blacklist + WHERE id IS NULL |
| 去重 | `SELECT DISTINCT` | `GROUP BY t1.logic_game_id, t1.name` |
| `bind` 顺序 | `channel, client_type, channel` | `client_type, client_type, channel` |
| 排序 | `ORDER BY ga.id` | **无 ORDER BY** |
| `cc_logic_game_version` | INNER JOIN | **保留** INNER JOIN（确保 version 有效） |

主要变化：**新 SQL 摆脱了 `channel → game_tag` 的间接映射**——旧版要先解析 channel 才能知道哪些 `cc_logic_game_wide` 命中，新版直接按 `client_type` 过滤 `cc_logic_game_wide`，再用 `cc_logic_game_exclude.channel = ?` 处理渠道级排除。

`bind` 顺序变更：旧为 `channel, client_type, channel`，新为 `client_type, client_type, channel`——所有调用方 `list_external_games_cached` 必须按新顺序传参。

## 作用

按 **`client_type` + `channel`** 列出**当前可玩**的逻辑游戏，过滤链：

1. 用 `client_type` 从 `cc_logic_game_wide` 拿全部宽表行
2. LEFT JOIN `cc_logic_game_exclude`（按 `client_type` + `channel` 排除）
3. INNER JOIN `cc_logic_game_version`（确保 version 有效）
4. LEFT JOIN `cc_logic_game_blacklist`（全局黑名单）
5. `WHERE t2.id IS NULL AND t4.id IS NULL` 保留未排除的
6. `GROUP BY` 去重
7. 外层 `INNER JOIN cc_logic_game` 取 `name` / `alias` 展示字段

## 参数

| 占位符 | 类型 | 出现 | 含义 |
| ------ | ---- | ---- | ---- |
| `?`    | `&str` | 1 | `cc_logic_game_wide.client_type` |
| `?`    | `&str` | 2 | `cc_logic_game_exclude.client_type` |
| `?`    | `&str` | 3 | `cc_logic_game_exclude.channel` |

代码里顺序为 `.bind(client_type).bind(client_type).bind(channel)`。

## 返回

| Rust 类型 | 描述 |
| --------- | ---- |
| `Result<Vec<(u32, String, String)>, AppError>` | `(cc_logic_game.id, name, alias)`，**无序** |

## 涉及的表 / 列

| 表 | 角色 | 关键列 |
| -- | ---- | ------ |
| `cc_logic_game_wide` | 过滤源（client_type 子查询） | `client_type`（过滤）/ `logic_game_id` / `name` / `version` |
| `cc_logic_game_exclude` | 渠道级排除 | `logic_game_id`（JOIN 键）/ `client_type`（过滤）/ `channel`（过滤） |
| `cc_logic_game_version` | 版本校验 | `version`（JOIN 键） |
| `cc_logic_game_blacklist` | 全局黑名单 | `logic_game_id`（JOIN 键） |
| `cc_logic_game` | 外层主表 | `id`（返回）/ `name`（返回）/ `alias`（返回） |

## 失败 / 边界

- `cc_logic_game_wide` 在该 `client_type` 下无任何记录 → 内层 0 行 → 外层 0 行 → 空 `Vec`。
- `cc_logic_game_version` 缺失 → INNER JOIN 失败 → 0 行。
- 全部命中 exclude 或 blacklist → 0 行。
- `cc_logic_game` 与 `cc_logic_game_wide` 失联（孤儿 wide 行）→ 0 行。
