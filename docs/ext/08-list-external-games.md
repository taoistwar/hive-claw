# 08 · `list_external_games` — 按客户端和渠道列出可用游戏

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::game_service::list_external_games` |
| 返回类型 | `Result<Vec<(i64, String, String)>, AppError>` |
| 源文件 | [`services/game_service.rs`](../../crates/hiveweb/src/services/game_service.rs) |
| 生产调用方 | builtin [`game_list`](../../crates/hiveweb/src/runtime/builtins/game_list.rs) |
| 缓存包装 | `list_external_games_cached`，已定义但当前生产代码**没有调用** |
| 缓存键 / TTL | 包装函数被调用时使用 `game_list:{channel}:{client_type}` / 600 秒，并会缓存空数组 |

> 当前 `game_list` builtin 直接查询外部数据库。此 SQL 不使用 `cc_promotion_channel`，也不执行 `channel → game_tag` 映射；`channel` 只参与排除表过滤。

> 本文中的 `cc_logic_game.alias` 是外部游戏数据，仍供 `game_list` builtin 使用；它与
> 已废弃的管理中心“游戏别名管理”（本地 `games` / `game_alias_entries` 及
> `/api/game-aliases*`）不是同一功能。

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
  INNER JOIN cc_logic_game_version t3
    ON t1.version = t3.version
  LEFT JOIN cc_logic_game_blacklist t4
    ON t1.logic_game_id = t4.logic_game_id
  WHERE t2.id IS NULL AND t4.id IS NULL
  GROUP BY t1.logic_game_id, t1.name
) z1
INNER JOIN (
  SELECT * FROM cc_logic_game WHERE status = 1
) z2 ON z1.logic_game_id = z2.id
```

## 作用与过滤链

1. 按 `client_type` 从 `cc_logic_game_wide` 取候选行；这里是直接比较，没有 `lower(...)`。
2. 按同一 `client_type + channel` 关联 `cc_logic_game_exclude`。
3. 通过 `cc_logic_game_version` INNER JOIN 校验版本存在。
4. 关联全局 `cc_logic_game_blacklist`。
5. 仅保留未命中 exclude 和 blacklist 的行。
6. 按 `logic_game_id + wide.name` 分组去重。
7. INNER JOIN `status = 1` 的 `cc_logic_game`，最终返回主表中的 ID、名称和别名。

SQL 不含 `ORDER BY`，因此结果顺序未定义。

## 参数与 bind 顺序

函数签名顺序是：

```rust
list_external_games(ext_pool, channel, client_type)
```

SQL bind 顺序与函数参数顺序不同：

| 次序 | 值 | 过滤位置 |
| ---- | -- | -------- |
| 1 | `client_type` | `cc_logic_game_wide.client_type` |
| 2 | `client_type` | `cc_logic_game_exclude.client_type` |
| 3 | `channel` | `cc_logic_game_exclude.channel` |

代码对应 `.bind(client_type).bind(client_type).bind(channel)`。

## 返回与 builtin 输出

服务函数返回：

```text
Vec<(i64, String, String)>
     id   name    alias
```

`alias` 为 NULL 时由 SQL 转成空字符串。`game_list` builtin 随后：

- 按逗号拆分 `alias`；
- 去除首尾空白、跳过空值并在单个游戏内去重；
- 生成结构化 `games: [{ id, name, aliases }]`；
- 同时生成适合模型展示的 `data` 文本；
- 把本次 `client_type` 写入 AgentContext 的 `target_client_type`。

## 空结果与错误

- 指定客户端没有宽表行：返回空 `Vec`。
- 版本缺失、命中 exclude / blacklist、或主表游戏不是 `status = 1`：相应游戏不返回。
- 全部被过滤：服务函数返回空 `Vec`；builtin 返回 `games: []` 和空字符串 `data`，没有专用“未找到”错误。
- SQL 失败：包装为 `AppError::Internal("game_list external query: ...")`，builtin 再转为执行错误。

## 缓存现状

`list_external_games_cached` 使用标准 cache-aside：Redis 读取失败或 miss 时查询 DB，查询成功后以 600 秒 TTL 写回，Redis 写失败不影响结果。它也会缓存空列表。

但是当前唯一生产调用方 `game_list` 使用的是 `list_external_games`，所以该 Redis wrapper 目前没有生产消费者。
