# 09 · `get_game_channels` — 取某游戏可用的渠道列表

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::game_service::get_game_channels` |
| 缓存包装 | **无**（调用频率低 + 业务侧要求实时） |
| 源文件 | [services/game_service.rs L418-438](../../crates/hiveweb/src/services/game_service.rs#L418-L438) |
| 调用方 | [`api/game.rs`](../../crates/hiveweb/src/api/game.rs) `POST /game/channels`（运营后台游戏详情页） |

## SQL 原文

```sql
SELECT t1.prom_channel
FROM (
    SELECT pc.prom_channel, pc.prom_platform, pc.game_tag, pc.department, pc.director
    FROM cc_logic_game_wide w
    INNER JOIN cc_promotion_channel pc
        ON pc.game_tag = w.channel_game_tag
       AND pc.status   = 1
    WHERE w.logic_game_id = ?
) t1
GROUP BY t1.prom_channel
```

## 作用

返回该游戏**当前可投放**的渠道集合（去重）。

逻辑：

1. 用 `w.logic_game_id = ?` 在 `cc_logic_game_wide` 找到该游戏的 `channel_game_tag`
2. INNER JOIN `cc_promotion_channel` 用 `game_tag` 找所有 `status = 1`（启用）的渠道
3. 子查询里冗余 SELECT 了 `prom_platform` / `game_tag` / `department` / `director` 是历史遗留（外部这些字段偶尔被业务侧参考），即使外层 `GROUP BY` 只用 `prom_channel`
4. `GROUP BY` 在外层做去重

## 参数

| 占位符 | 类型 | 含义 |
| ------ | ---- | ---- |
| `?`    | `i64` | `cc_logic_game_wide.logic_game_id` |

## 返回

| Rust 类型 | 描述 |
| --------- | ---- |
| `Result<Vec<String>, AppError>` | 渠道码数组（`String`） |

> 错误用 `AppError::Internal(format!("game_channels query: {e}"))` 包装。

## 涉及的表 / 列

| 表 | 关键列 |
| -- | ------ |
| `cc_logic_game_wide` | `logic_game_id`（过滤）/ `channel_game_tag`（JOIN 键） |
| `cc_promotion_channel` | `game_tag`（JOIN 键）/ `status = 1`（过滤）/ `prom_channel`（返回） |

## 失败 / 边界

- `logic_game_id` 无对应 `cc_logic_game_wide` → 内层空 → `GROUP BY` 后空 → 返回 `Vec::new()`。
- `status = 1` 是硬编码——下架渠道（status=0/2 等）被排除。
- 不缓存，命中频繁性低；调用方都是后台管理接口。
