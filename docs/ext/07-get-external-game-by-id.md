# 07 · `get_external_game_by_id` — 复合查询：按 logic_game_id + client + channel 取游戏详情

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::game_service::get_external_game_by_id` |
| 缓存包装 | `get_external_game_by_id_cached`（`cached_or_fetch`） |
| 缓存键 | `game_info:{logic_game_id}:{client_type}:{channel}` |
| 缓存 TTL | 900 s（15 min） |
| 源文件 | [services/game_service.rs L373-414](../../crates/hiveweb/src/services/game_service.rs#L373-L414) |
| 调用方 | builtin [`game_info`](../../crates/hiveweb/src/runtime/builtins/game_info.rs)（Agent 询问某游戏详情时） |
| 返回结构体 | [`ExternalGameInfo`](../../crates/hiveweb/src/services/game_service.rs#L359-L371) |

## 结构体 `ExternalGameInfo`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ExternalGameInfo {
    pub logic_game_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub cover_image: Option<String>,
    pub game_tags: Option<serde_json::Value>,
    pub computer_id: Option<i64>,
    pub platform_name: Option<String>,
    pub client_type: Option<String>,
    pub channel: Option<String>,
}
```

## SQL 原文

```sql
SELECT
    t1.logic_game_id, t1.name, t1.description, t1.cover_image, t1.game_tags,
    t2.computer_id,
    t3.name AS platform_name,
    t1.client_type,
    t5.prom_channel AS channel
FROM (
    SELECT * FROM cc_logic_game_wide WHERE logic_game_id = ?
) t1
INNER JOIN (
    SELECT * FROM cc_game WHERE logic_game_id = ?
) t2 ON t1.logic_game_id = t2.logic_game_id
INNER JOIN cc_game_platform          t3 ON t2.platform    = t3.code
INNER JOIN cc_logic_game_version     t4 ON t1.version     = t4.version
INNER JOIN (
    SELECT pc.id, pc.game_tag, pc.prom_channel
    FROM cc_promotion_channel pc
    WHERE pc.prom_channel = ?
) t5 ON t1.channel_game_tag = t5.game_tag
LEFT JOIN (
    SELECT * FROM cc_logic_game_exclude
    WHERE client_type = ? AND channel = ?
) t6 ON t1.logic_game_id = t6.logic_game_id
LEFT JOIN cc_logic_game_blacklist    t7 ON t1.logic_game_id = t7.logic_game_id
WHERE t6.id IS NULL
  AND t7.id IS NULL
```

## ⚠️ 最近重大变更

| 维度 | 旧 | 新 |
| ---- | -- | -- |
| 入参 | `(ext_pool, game_id)` | **`(ext_pool, logic_game_id, client_type, channel)`** |
| 入参类型 | `game_id: i64`（`cc_game.id`） | `logic_game_id: i64`（`cc_logic_game.id`）+ `client_type: &str` + `channel: &str` |
| 返回类型 | `Option<(i64, String, String)>` | **`Vec<ExternalGameInfo>`**（9 字段结构体） |
| 缓存键 | `game_info:{game_id}` | `game_info:{logic_game_id}:{client_type}:{channel}` |
| SQL 复杂度 | 2 表 JOIN（`cc_logic_game` + `cc_game`） | **7 表 JOIN**（含子查询） |
| 行数语义 | `fetch_optional`（至多 1 行） | `fetch_all`（可能多行：同一 logic_game 可挂多个 cc_game） |
| 排除过滤 | 无 | `cc_logic_game_exclude` (client_type + channel) + `cc_logic_game_blacklist` |

**为什么 `Vec` 不是 `Option`**：`cc_logic_game_wide` 是按 `logic_game_id` 过滤，但下游会 INNER JOIN `cc_game`（按 `logic_game_id` 匹配），同一个逻辑游戏可能在不同 `platform` 下有多个 `cc_game` 条目——故可能返回多行。

**排除逻辑下推**：把 `cc_logic_game_exclude` 与 `cc_logic_game_blacklist` 的过滤推到 SQL 末尾的 `WHERE`，LEFT JOIN 命中即排除。

## 作用

按 **`logic_game_id` + `client_type` + `channel`** 复合键返回游戏的**完整展示信息**，包括：

- 基础元数据（名称、描述、封面、标签）
- 关联的电脑/平台信息（`computer_id`, `platform_name`）
- 客户端 / 渠道上下文（`client_type`, `channel`）

**注意**：未配置 `EXTERNAL_DB_URL` 时 builtin 仍会调用本函数（由 `ext_pool: Option<…>` 决定是否报错），但此处代码本身不检查 `ext_pool` 是否为 `None`——前提是调用方确保池已注入。

## 参数

| 占位符 | 类型 | 出现 | 含义 |
| ------ | ---- | ---- | ---- |
| `?`    | `i64` | 1 | `cc_logic_game_wide.logic_game_id` 子查询内 |
| `?`    | `i64` | 2 | `cc_game.logic_game_id` 子查询内 |
| `?`    | `&str` | 3 | `cc_promotion_channel.prom_channel` 子查询内 |
| `?`    | `&str` | 4 | `cc_logic_game_exclude.client_type` 子查询内 |
| `?`    | `&str` | 5 | `cc_logic_game_exclude.channel` 子查询内 |

代码里顺序为 `.bind(logic_game_id).bind(logic_game_id).bind(channel).bind(client_type).bind(channel)`。

## 返回

| Rust 类型 | 描述 |
| --------- | ---- |
| `Result<Vec<ExternalGameInfo>, AppError>` | 0..N 行游戏详情；空 `Vec` = 不存在 / 全部被排除 / 渠道未配置 |

> 错误用 `AppError::Internal(format!("game_info external query: {e}"))` 包装。

## 涉及的表 / 列

| 表 | 角色 | 关键列 |
| -- | ---- | ------ |
| `cc_logic_game_wide` | 主表子查询 | `logic_game_id`（过滤）/ `name` / `description` / `cover_image` / `game_tags` / `client_type` / `version` / `channel_game_tag` |
| `cc_game` | 上架条目子查询 | `logic_game_id`（JOIN + 过滤）/ `computer_id` / `platform`（JOIN 到 platform） |
| `cc_game_platform` | 平台字典 | `code`（JOIN 键）/ `name`（`platform_name`） |
| `cc_logic_game_version` | 版本校验 | `version`（JOIN 键） |
| `cc_promotion_channel` | 渠道子查询 | `prom_channel`（过滤）/ `game_tag`（JOIN 键） |
| `cc_logic_game_exclude` | 渠道级排除子查询 | `logic_game_id`（JOIN 键）/ `client_type`（过滤）/ `channel`（过滤） |
| `cc_logic_game_blacklist` | 全局黑名单 | `logic_game_id`（JOIN 键） |

## 失败 / 边界

- `logic_game_id` 不存在 → 内层子查询空 → 全 0 行 → 空 `Vec`。
- `channel` 在 `cc_promotion_channel` 找不到 → INNER JOIN t5 失败 → 0 行。
- `cc_logic_game_exclude` 命中 → `t6.id` 非 NULL → 该 game 被排除。
- `cc_logic_game_blacklist` 命中 → 0 行。
- 同一 `logic_game_id` 在 `cc_game` 有 N 个 platform 条目 → 返回 N 行。
