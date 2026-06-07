# Data Model: 敏感词过滤

**Feature**: 010-sensitive-word-filter | **Date**: 2026-06-07

## Entities

### SensitiveWord（敏感词条目）

持久化存储的敏感词配置记录。

| Field | Type | Constraints | Description |
|-------|------|-------------|-------------|
| `id` | BIGINT | PK, AUTO_INCREMENT | 主键 |
| `word` | VARCHAR(512) | NOT NULL | 敏感词文本（精确模式）或正则表达式（正则模式） |
| `match_mode` | VARCHAR(16) | NOT NULL, DEFAULT 'exact' | 匹配模式：`exact` 或 `regex` |
| `enabled` | TINYINT(1) | NOT NULL, DEFAULT 1 | 是否启用：0=禁用, 1=启用 |
| `created_at` | DATETIME | NOT NULL, DEFAULT CURRENT_TIMESTAMP | 创建时间 |
| `updated_at` | DATETIME | NOT NULL, DEFAULT CURRENT_TIMESTAMP ON UPDATE | 更新时间 |

**Uniqueness**: `(word, match_mode)` 联合唯一索引，防止重复添加同一模式的同一词条。

**Validation**:
- `word`：非空，长度 ≤ 512
- `match_mode`：必须为 `exact` 或 `regex`
- 当 `match_mode = regex` 时，保存前校验正则合法性（`regex::Regex::new(word).is_ok()`）

**Lifecycle**: 无复杂状态转换。创建后默认启用，管理员可禁用/启用/删除。禁用仅影响过滤行为，不删除数据库记录。

### SensitivePattern（内存缓存条目）

内存中缓存的已编译匹配规则，由 `SensitiveFilter` 管理。

| Attribute | Type | Description |
|-----------|------|-------------|
| `id` | i64 | 对应 DB id，用于日志追溯 |
| `word` | String | 原始文本（用于日志） |
| `pattern` | Pattern enum | 已编译的匹配模式：`Exact(String)` 或 `Regex(regex::Regex)` |

**Lifecycle**: 启动时从 DB 加载所有 `enabled=1` 的记录并编译。缓存刷新时整体替换。不需要部分更新。

### FilterLog（过滤日志）

过滤事件的审计记录。

| Field | Type | Description |
|-------|------|-------------|
| `id` | BIGINT | PK, AUTO_INCREMENT |
| `event_type` | VARCHAR(16) | `input_block`（输入拦截）或 `output_replace`（输出替换） |
| `user_id` | BIGINT | 触发用户 ID（输入拦截时有值） |
| `session_id` | BIGINT | 关联会话 ID（输出替换时有值） |
| `triggered_word` | VARCHAR(512) | 命中的敏感词原始文本 |
| `created_at` | DATETIME | 事件时间 |

**Retention**: 暂不设置自动清理策略，后续按需加入。日志写入为 fire-and-forget（不影响主流程）。

## Relationships

```
SensitiveWord (DB) ───加载───> SensitivePattern (Memory Cache)
                                      │
                                      ▼
                               SensitiveFilter
                                      │
                          ┌───────────┴───────────┐
                          ▼                       ▼
                    input check              output check
                          │                       │
                          ▼                       ▼
                     FilterLog               FilterLog
                   (event_type=            (event_type=
                    input_block)           output_replace)
```

## DB Migration

```sql
CREATE TABLE sensitive_words (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    word VARCHAR(512) NOT NULL,
    match_mode VARCHAR(16) NOT NULL DEFAULT 'exact',
    enabled TINYINT(1) NOT NULL DEFAULT 1,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    UNIQUE KEY uk_word_mode (word, match_mode)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE sensitive_filter_logs (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    event_type VARCHAR(16) NOT NULL,
    user_id BIGINT,
    session_id BIGINT,
    triggered_word VARCHAR(512) NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    INDEX idx_event_type (event_type),
    INDEX idx_created_at (created_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
```
