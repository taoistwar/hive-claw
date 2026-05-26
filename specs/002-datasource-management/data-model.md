# Data Model: 数据源管理

## Entities

### DataSource

Represents a MySQL database connection configuration.

| Field | Type | Constraints | Description |
|-------|------|-------------|-------------|
| `id` | `i64` | PK, AUTO_INCREMENT | 数据源唯一标识 |
| `name` | `String` | NOT NULL, UNIQUE, max 128 chars | 用户自定义名称（如 "生产库"） |
| `host` | `String` | NOT NULL, max 255 chars | MySQL 主机地址（IP 或域名） |
| `port` | `u16` | NOT NULL, default 3306 | MySQL 端口 |
| `username` | `String` | NOT NULL, max 128 chars | MySQL 用户名 |
| `encrypted_password` | `Vec<u8>` | NOT NULL | 加密后的密码（二进制） |
| `created_at` | `DateTime<Utc>` | NOT NULL | 创建时间 |
| `updated_at` | `DateTime<Utc>` | NOT NULL | 最后更新时间 |

**Validation rules**:
- `name` 不能为空，不能重复
- `host` 必须是有效的 IP 地址或域名
- `port` 范围 1-65535
- 连接测试必须成功后才能保存

### DatabaseInfo

Read-only metadata about a MySQL database (schema). Not persisted — fetched live from MySQL.

| Field | Type | Description |
|-------|------|-------------|
| `name` | `String` | 数据库名称 |
| `charset` | `Option<String>` | 字符集 |
| `collation` | `Option<String>` | 排序规则 |

### TableInfo

Read-only metadata about a MySQL table. Not persisted — fetched live from MySQL.

| Field | Type | Description |
|-------|------|-------------|
| `name` | `String` | 表名 |
| `comment` | `Option<String>` | 表注释 |
| `engine` | `Option<String>` | 存储引擎（如 InnoDB） |
| `row_count` | `Option<i64>` | 估算行数 |

### ColumnInfo

Read-only metadata about a MySQL table column. Not persisted — fetched live from MySQL.

| Field | Type | Description |
|-------|------|-------------|
| `name` | `String` | 列名 |
| `data_type` | `String` | 数据类型（如 VARCHAR, INT） |
| `is_nullable` | `bool` | 是否允许 NULL |
| `column_default` | `Option<String>` | 默认值 |
| `is_primary_key` | `bool` | 是否为主键 |
| `comment` | `Option<String>` | 列注释 |
| `character_maximum_length` | `Option<i64>` | 字符最大长度 |
| `numeric_precision` | `Option<i64>` | 数值精度 |

### TableData

Query result from a table's data preview.

| Field | Type | Description |
|-------|------|-------------|
| `columns` | `Vec<String>` | 列名列表 |
| `rows` | `Vec<Vec<Option<String>>>` | 数据行，每行是字符串值的可选列表 |
| `total_count` | `i64` | 符合条件的总行数 |
| `limit` | `i64` | 当前查询限制行数 |
| `offset` | `i64` | 当前偏移量 |

## Relationships

```text
DataSource (1) ──▶ (N) DatabaseInfo [live query]
DatabaseInfo (1) ──▶ (N) TableInfo [live query]
TableInfo (1) ──▶ (N) ColumnInfo [live query]
TableInfo (1) ──▶ (N) TableData [live query]
```

- `DataSource` 是唯一持久化的实体（SQLite）
- `DatabaseInfo`、`TableInfo`、`ColumnInfo`、`TableData` 均为运行时从 MySQL 动态获取

## SQLite Schema

```sql
CREATE TABLE IF NOT EXISTS data_sources (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    host TEXT NOT NULL,
    port INTEGER NOT NULL DEFAULT 3306,
    username TEXT NOT NULL,
    encrypted_password BLOB NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_data_sources_name ON data_sources(name);
```

**Database file location**: `~/.local/share/hivegui/datasources.db`

## State Transitions

### DataSource lifecycle

```text
[Created] ──(test connection passes)──▶ [Verified] ──(save)──▶ [Persisted]
[Persisted] ──(edit)──▶ [Modified] ──(test passes)──▶ [Persisted]
[Persisted] ──(delete)──▶ [Deleted]
```
