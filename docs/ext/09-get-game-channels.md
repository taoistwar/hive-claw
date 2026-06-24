# 09 · `get_game_channels` — 取某游戏可用的**启用**渠道列表

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::game_service::get_game_channels` |
| 缓存包装 | **无**（调用频率低 + 业务侧要求实时） |
| 源文件 | [services/game_service.rs L455-475](../../crates/hiveweb/src/services/game_service.rs#L455-L475) |
| 调用方 | [`api/game.rs`](../../crates/hiveweb/src/api/game.rs) `POST /game/channels`（运营后台游戏详情页） |

## SQL 原文

```sql
SELECT DISTINCT t2.prom_channel
FROM (
    SELECT * FROM cc_logic_game_wide WHERE logic_game_id = ?
) t1
INNER JOIN cc_promotion_channel t2
    ON t2.game_tag = t1.channel_game_tag
   AND t2.status  = 1
LEFT JOIN (
    SELECT * FROM cc_logic_game_exclude
) t3 ON t1.logic_game_id = t3.logic_game_id
WHERE t3.id IS NULL
```

## ⚠️ 最近变更

| 维度 | 旧 | 新 |
| ---- | -- | -- |
| 结构 | 嵌套子查询 + `GROUP BY t1.prom_channel` | 平铺 FROM + `LEFT JOIN cc_logic_game_exclude` + `WHERE t3.id IS NULL` |
| 去重 | 内层 `GROUP BY t1.prom_channel` | 外层 `SELECT DISTINCT t2.prom_channel` |
| 排除过滤 | 无 | 新增 `LEFT JOIN cc_logic_game_exclude` + `t3.id IS NULL` 过滤 |
| `bind` 数 | 1（不变） | 1（不变） |

主要变化：**新增 `cc_logic_game_exclude` 过滤**——旧版只查"该游戏在哪些启用渠道"，新版会排除"在该游戏上配置了 exclude 的渠道"。

> 注意新版 `t3` 子查询没有 `client_type`/`channel` 过滤——当前 SQL 是"该游戏被任意配置 exclude 的渠道全部排除"，这是**潜在的过度过滤**（应按 `logic_game_id` 精匹配，但当前实现是全表扫 exclude 匹配 logic_game_id）。等后续业务反馈再决定是否加严。

## 作用

返回该游戏**当前可投放**的渠道集合（去重）。逻辑：

1. 在 `cc_logic_game_wide` 找到 `logic_game_id = ?` 的 `channel_game_tag`
2. INNER JOIN `cc_promotion_channel` 用 `game_tag` 找 `status = 1`（启用）的渠道
3. LEFT JOIN `cc_logic_game_exclude`，若该 game 被 exclude 配置命中 → 整行排除
4. `DISTINCT` 去重

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
| `cc_logic_game_exclude` | `logic_game_id`（JOIN 键，无其它过滤） |

## 失败 / 边界

- `logic_game_id` 无对应 `cc_logic_game_wide` → 内层空 → 0 行 → `Vec::new()`。
- `status = 1` 是硬编码——下架渠道（status=0/2 等）被排除。
- 任意 `cc_logic_game_exclude` 命中 → LEFT JOIN 不为 NULL → `t3.id IS NULL` 失败 → 该行被排除。
- 不缓存，命中频繁性低；调用方都是后台管理接口。
