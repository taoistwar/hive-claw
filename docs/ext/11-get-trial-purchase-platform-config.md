# 11 · `get_trial_purchase_platform_config` — 查询试玩购买平台优先级

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::game_service::get_trial_purchase_platform_config` |
| 缓存包装 | **无** |
| 源文件 | [services/game_service.rs](../../crates/hiveweb/src/services/game_service.rs) |
| 直接调用方 | `services::game_service::sort_external_games_by_priority` |

## SQL 原文

```sql
SELECT content
FROM cc_config
WHERE label = 'trialPurchasePlatformConfig'
LIMIT 1
```

## 作用

从外部数据库 `cc_config` 表读取试玩购买平台配置。当前排序逻辑只使用配置中的
`platformPriority` 字段，预期结构为：

```json
{
  "platformPriority": ["平台 A", "平台 B"]
}
```

`sort_external_games_by_priority` 按数组位置对 `ExternalGameInfo.platform_name` 排序。
配置中出现的平台排在前面；未配置或 `platform_name = NULL` 的游戏排在后面。调用方
随后通常取第一项，作为同一游戏 ID 下优先级最高的平台记录。

## 配置标签与状态

| 项 | 当前行为 |
| -- | -------- |
| `label` | 精确匹配 `trialPurchasePlatformConfig`，大小写行为取决于数据库列的 collation |
| `status` | **没有过滤**；SQL 不包含 `status = 'ACTIVE'` |
| 重复配置 | `LIMIT 1` 且没有 `ORDER BY`，存在重复 label 时不保证选中哪一行 |

这与 `AIassistantChatLimitConfig` 等其他 `cc_config` 查询不同：即使该表有
`status` 字段，本函数也可能读到非 `ACTIVE` 记录。

## 参数

无参数。配置 label 直接写在 SQL 中。

## 返回

| Rust 类型 | 描述 |
| --------- | ---- |
| `Result<Option<serde_json::Value>, AppError>` | `Some(content)` 表示读取到非 NULL JSON；无记录或 `content IS NULL` 时为 `None` |

查询或 JSON 解码失败时返回
`AppError::Internal("cc_config query: …")`。无效 JSON 不会被
`serde_json::Value` 自动容忍。

## 实际调用与降级

本函数没有独立 HTTP 接口，只由 `sort_external_games_by_priority` 直接调用。排序逻辑
间接用于：

- `get_single_external_game_info`：运行时 `game_info` builtin 和 Legacy
  `POST /api/recommended-games/execute` 兼容接口查询单个游戏时，选择优先平台；后者
  已废弃，新代码不得调用或依赖。
- `api/chat_messages.rs`：刷新历史消息中的游戏卡片时，选择优先平台。
- `get_single_external_game_info_cached`：数据缓存未命中并重新查询外部游戏时，也会执行
  平台排序；当前仓库没有该包装函数的调用点。

配置本身没有 Redis 缓存。排序函数按以下方式静默降级：

- 查询失败、无记录或 `content` 为 NULL：使用空优先级，不调整原列表顺序。
- `platformPriority` 缺失或不是数组：使用空优先级，不排序。
- 数组中的非字符串元素：直接忽略。

`sort_external_games_by_priority` 使用 `.unwrap_or(None)` 丢弃配置查询错误，目前不会把
该错误返回给上层，也不会记录日志。

## 涉及的表 / 列

| 表 | 列 | 用途 |
| -- | -- | ---- |
| `cc_config` | `label` | 固定配置键 `trialPurchasePlatformConfig` |
| `cc_config` | `content` | JSON 配置，读取 `platformPriority` 字符串数组 |
| `cc_config` | `status` | 当前查询未使用 |

## 失败 / 边界

- `platformPriority` 为空数组时不会排序。
- 平台名比较是大小写敏感的 Rust 字符串精确匹配；名称不一致时视为未配置平台。
- 多个平台都未配置时排序键相同，保留它们原有的相对次序。
- 因为没有独立缓存，每次实际执行排序都会查询一次 `cc_config`；外层游戏数据缓存命中
  时不会进入排序，也不会查询该配置。
