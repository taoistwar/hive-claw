# 10 · `get_game_client_types` — 取某游戏的客户端类型

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::game_service::get_game_client_types` |
| 缓存包装 | **无**（同 09） |
| 源文件 | [services/game_service.rs L441-453](../../crates/hiveweb/src/services/game_service.rs#L441-L453) |
| 调用方 | [`api/game.rs`](../../crates/hiveweb/src/api/game.rs) `POST /game/client_types`（运营后台游戏详情页） |

## SQL 原文

```sql
SELECT client_type FROM cc_logic_game_wide WHERE logic_game_id = ?
```

## 作用

返回某个 `logic_game_id` 在 `cc_logic_game_wide` 中的 `client_type` 字段。该字段是 JSON 数组（如 `["pc","mobile"]`），由调用方在 `serde_json::Value` 中继续解析。

## 参数

| 占位符 | 类型 | 含义 |
| ------ | ---- | ---- |
| `?`    | `i64` | `cc_logic_game_wide.logic_game_id` |

## 返回

| Rust 类型 | 描述 |
| --------- | ---- |
| `Result<Option<serde_json::Value>, AppError>` | `Some(Value)`（JSON 数组 / 字符串等）或 `None`（记录不存在） |

> 错误用 `AppError::Internal(format!("game_client_types query: {e}"))` 包装。

## 表结构

| 表 | 列 | 用途 |
| -- | -- | ---- |
| `cc_logic_game_wide` | `logic_game_id` | 过滤主键 |
| `cc_logic_game_wide` | `client_type` | JSON 字段，记录支持的客户端类型集合 |

## 失败 / 边界

- 无 `cc_logic_game_wide` 记录 → `fetch_optional` 返回 `None`。
- `client_type` 是 NULL（SQL 仍返回一行） → 映射成 `Some(Option<None>)` → 业务侧 `Some(None)` → 需调用方处理。
