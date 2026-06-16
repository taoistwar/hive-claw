# 07 · `get_external_game_by_id` — 按 cc_game.id 查 cc_logic_game

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::game_service::get_external_game_by_id` |
| 缓存包装 | `get_external_game_by_id_cached`（`cached_or_fetch`） |
| 缓存键 | `game_info:{game_id}` |
| 缓存 TTL | 900 s（15 min） |
| 源文件 | [services/game_service.rs L360-373](../../crates/hiveweb/src/services/game_service.rs#L360-L373) |
| 调用方 | builtin [`game_info`](../../crates/hiveweb/src/runtime/builtins/game_info.rs)（Agent 询问某游戏详情时） |

## SQL 原文

```sql
SELECT cast(t2.id as SIGNED), t1.name, COALESCE(t1.alias, '') AS alias
FROM cc_logic_game t1
LEFT JOIN cc_game t2 ON t1.id = t2.logic_game_id
WHERE t2.id = ?
```

## ⚠️ 最近变更

返回列由 `t1.id`（`cc_logic_game.id`）**改为** `cast(t2.id as SIGNED)`（`cc_game.id` 转有符号整型）。

原 SQL 用 `t1.id` 会返回**逻辑游戏**的 ID，但调用方期望的是用户实际请求的 `cc_game.id`（在 WHERE 条件里就是用 `t2.id` 过滤的）。改成 `cast(t2.id as SIGNED)` 后：

- 语义上自洽：返回的就是**过滤键对应的 ID**（即 `cc_game.id`），不再跨表误用 `cc_logic_game.id`。
- `cast(... as SIGNED)` 是 MySQL 显式类型转换——外部库 `cc_game.id` 可能是 `UNSIGNED`/`BIGINT`，转为 `SIGNED` 后才能安全映射到 Rust 的 `i64`。

## 作用

外部库把"游戏"拆成两层：

- `cc_logic_game`：逻辑游戏（一份玩法）
- `cc_game`：具体渠道上架的"游戏条目"（不同渠道的 ID 不同）

本 SQL 通过 `cc_game.id` 反查它所属的 `cc_logic_game`，返回**业务展示名 `(id, name, alias)`**。

## 参数

| 占位符 | 类型 | 含义 |
| ------ | ---- | ---- |
| `?`    | `i64` | `cc_game.id`（外部库的具体上架条目 ID） |

## 返回

| Rust 类型 | 描述 |
| --------- | ---- |
| `Result<Option<(i64, String, String)>, AppError>` | `(cc_logic_game.id, name, alias)`；找不到 → `None` |

> 错误已用 `AppError::Internal(format!("game_info external query: {e}"))` 包装。

## 涉及的表 / 列

| 表 | 关键列 |
| -- | ------ |
| `cc_logic_game` | `id`（返回）/ `name`（返回）/ `alias`（返回，COALESCE 防 NULL） |
| `cc_game` | `id`（过滤）/ `logic_game_id`（JOIN 键） |

## 失败 / 边界

- `cc_game.id` 不存在 → `fetch_optional` 返回 `None`。
- `alias` 为 NULL → COALESCE 兜底为空串。
- 库不可达 → `AppError::Internal` 向上抛。
