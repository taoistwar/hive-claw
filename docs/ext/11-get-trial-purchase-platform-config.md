# 11 · `get_trial_purchase_platform_config` — 试玩购买平台配置

## 元数据

| 字段 | 值 |
| ---- | -- |
| Rust 函数 | `services::game_service::get_trial_purchase_platform_config` |
| 缓存包装 | **无**（本次新增时未引入缓存） |
| 源文件 | [services/game_service.rs L505-515](../../crates/hiveweb/src/services/game_service.rs#L505-L515) |
| 调用方 | 运营 / Agent 流程中按需查询 `trialPurchasePlatformConfig` 配置项 |

## SQL 原文

```sql
SELECT content FROM cc_config WHERE label = 'trialPurchasePlatformConfig' LIMIT 1
```

## 作用

从外部库 `cc_config` 通用配置表中按 `label` 取出**试玩购买平台配置**的 JSON 内容。

`cc_config` 是外部库的"键值对 JSON 配置"通用表——`label` 是 key，`content` 是 JSON value。此函数硬编码 key 为 `trialPurchasePlatformConfig`，无入参。

`LIMIT 1` 是防御性写法——理论上 `label` 应唯一，但若外部库有重复行也只取第一行。

## 参数

无参数。`label` 是字面量。

## 返回

| Rust 类型 | 描述 |
| --------- | ---- |
| `Result<Option<serde_json::Value>, AppError>` | `Some(content)`（配置 JSON）或 `None`（未配置） |

> 错误用 `AppError::Internal(format!("cc_config query: {e}"))` 包装。

## 表结构

| 列 | 类型 | 用途 |
| -- | ---- | ---- |
| `label` | `VARCHAR` | 配置 key（此处硬编码为 `trialPurchasePlatformConfig`） |
| `content` | JSON / TEXT | 配置 value（返回的 JSON 树） |

## 失败 / 边界

- 无 `label = 'trialPurchasePlatformConfig'` 记录 → `fetch_optional` 返回 `None`。
- `content` 是 NULL → `Some(Option<None>)` → 业务侧按"空配置"处理。
- JSON 解析失败 → 由 `serde_json::Value` 自动容忍（非严格解析，结构异常时仍能拿到 Value）。

## 后续建议

- **建议加缓存**：配置项一般变更频率低；可参考 `cache_helper::TTL_CONFIG`（3600s）做 cache-aside。
- **建议硬编码 label 提到常量**：方便后续配置项扩张。
