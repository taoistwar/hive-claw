# 10 · `get_game_client_types` — 取某游戏可用客户端类型

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::game_service::get_game_client_types` |
| 缓存包装 | **无**（同 09） |
| 源文件 | [services/game_service.rs L478-501](../../crates/hiveweb/src/services/game_service.rs#L478-L501) |
| 调用方 | [`api/game.rs`](../../crates/hiveweb/src/api/game.rs) `POST /game/client_types`（运营后台游戏详情页） |

## SQL 原文

```sql
SELECT  t1.client_type
FROM (
    SELECT * FROM cc_logic_game_wide WHERE logic_game_id = ?
) t1
LEFT JOIN (
    SELECT * FROM cc_logic_game_exclude WHERE logic_game_id = ?
) t2 ON t1.logic_game_id = t2.logic_game_id
INNER JOIN cc_logic_game_version t3 ON t1.version = t3.version
LEFT JOIN cc_logic_game_blacklist t4 ON t1.logic_game_id = t4.logic_game_id
WHERE t2.id IS NULL AND t4.id IS NULL
GROUP BY t1.client_type
```

## ⚠️ 最近变更

| 维度 | 旧 | 新 |
| ---- | -- | -- |
| SQL 形态 | 单表直查 `SELECT client_type FROM cc_logic_game_wide WHERE logic_game_id = ?` | 子查询 + 3 表 JOIN（exclude + version + blacklist）+ GROUP BY |
| 排除过滤 | 无 | 新增 `cc_logic_game_exclude` + `cc_logic_game_blacklist` 过滤 |
| 版本校验 | 无 | 新增 `INNER JOIN cc_logic_game_version` |
| `bind` 数 | **1** | **2**（`cc_logic_game_wide.logic_game_id` + `cc_logic_game_exclude.logic_game_id`） |
| 去重 | 无 | `GROUP BY t1.client_type` |
| 排序 | 无 | 无（但 GROUP BY 隐式按 client_type 升序） |

## 作用

返回某个 `logic_game_id` 在 `cc_logic_game_wide` 中**有效**的 `client_type` 字段集合。该字段是 JSON 数组（如 `["pc","mobile"]`），由调用方在 `serde_json::Value` 中继续解析。

新 SQL 在原版基础上**叠加了 `cc_logic_game_exclude` + `cc_logic_game_blacklist` 排除逻辑 + `cc_logic_game_version` 版本校验**——与 08、09 保持一致。

## 参数

| 占位符 | 类型 | 出现 | 含义 |
| ------ | ---- | ---- | ---- |
| `?`    | `i64` | 1 | `cc_logic_game_wide.logic_game_id` |
| `?`    | `i64` | 2 | `cc_logic_game_exclude.logic_game_id` |

## 返回

| Rust 类型 | 描述 |
| --------- | ---- |
| `Result<Option<serde_json::Value>, AppError>` | `Some(Value)`（JSON 数组 / 字符串等）或 `None`（记录不存在） |

> 错误用 `AppError::Internal(format!("game_client_types query: {e}"))` 包装。

## 涉及的表 / 列

| 表 | 角色 | 关键列 |
| -- | ---- | ------ |
| `cc_logic_game_wide` | 过滤源子查询 | `logic_game_id`（过滤）/ `client_type`（返回 + GROUP BY）/ `version` |
| `cc_logic_game_exclude` | 排除子查询 | `logic_game_id`（过滤 + JOIN 键） |
| `cc_logic_game_version` | 版本校验 | `version`（JOIN 键） |
| `cc_logic_game_blacklist` | 全局黑名单 | `logic_game_id`（JOIN 键） |

## 失败 / 边界

- 无 `cc_logic_game_wide` 记录 → 内层 0 行 → `fetch_optional` 返回 `None`。
- 全部命中 exclude / blacklist / version 缺失 → 0 行 → `None`。
- `client_type` 是 NULL（SQL 仍返回一行） → 映射成 `Some(Option<None>)` → 业务侧 `Some(None)` → 需调用方处理。
- `GROUP BY t1.client_type` 把同值的 client_type 合并为一行——若同 `logic_game_id` 在 wide 表有多个不同 `client_type` 的 wide 行，将返回 1 行。
