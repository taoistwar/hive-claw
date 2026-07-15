# Data Model: Game Alias Management

> **已废弃（Legacy，2026-07-14）：** 本规格描述的游戏别名管理前后端已停止业务使用，仅为兼容历史数据、接口和审计记录保留。不得以本文档为依据新增调用、部署或功能扩展；下文仅作为历史设计档案。

**Created**: 2026-06-01  
**Feature**: Game Alias Management (006-game-alias-management)

## Entities

### 1. Game (游戏)

**Description**: 代表一个游戏实体，包含游戏的基本信息和时间戳。

**Fields**:
- `id`: i64 (主键，自增，从 1 开始)
- `name`: String (唯一，1-50 字符，索引)
- `created_at`: DateTime (UTC，创建时间)
- `updated_at`: DateTime (UTC，最后修改时间)

**Constraints**:
- name 唯一索引
- name 长度：1-50 字符
- created_at 不可修改
- updated_at 随每次编辑自动更新

**Relationships**:
- 一对多：Game → GameAliasEntry（一个游戏有多个别名）

**Validation Rules**:
- 游戏名称唯一
- 名称长度 1-50 字符

### 2. GameAliasEntry (游戏别名条目)

**Description**: 代表一个游戏的一个别名，每个别名单独一行存储。

**Fields**:
- `id`: i64 (主键，自增)
- `game_id`: i64 (外键，关联 Game.id)
- `alias`: String (唯一，1-50 字符，索引)

**Constraints**:
- alias 唯一索引（全局唯一，不同游戏不能有相同别名）
- game_id 外键约束：`ON DELETE CASCADE`（删除游戏时自动删除所有别名）
- alias 长度：1-50 字符
- alias 非空字符串

**Relationships**:
- 多对一：GameAliasEntry → Game（多个别名属于一个游戏）

**Validation Rules**:
- alias 不能为空字符串
- alias 在整个系统中唯一（跨游戏去重）
- 每个游戏最多 20 个别名（应用层验证）

## Database Schema

### SQL DDL (MySQL 8.0+)

```sql
-- 游戏表
CREATE TABLE games (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    name VARCHAR(50) NOT NULL UNIQUE COMMENT '游戏名称',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    INDEX idx_name (name)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='游戏表';

-- 游戏别名条目表
CREATE TABLE game_alias_entries (
    id BIGINT PRIMARY KEY AUTO_INCREMENT,
    game_id BIGINT NOT NULL COMMENT '所属游戏 ID',
    alias VARCHAR(50) NOT NULL UNIQUE COMMENT '别名字符串（全局唯一）',
    INDEX idx_game_id (game_id),
    INDEX idx_alias (alias),
    CONSTRAINT fk_game_alias_game FOREIGN KEY (game_id) REFERENCES games(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci COMMENT='游戏别名条目表';
```

## Rust Structs

### Game Model

```rust
#[derive(Debug, Clone, FromRow)]
pub struct Game {
    pub id: i64,
    pub name: String,
    pub aliases: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}
```

### DTOs (Data Transfer Objects)

```rust
// 请求 DTOs
#[derive(Debug, Deserialize)]
pub struct CreateGameRequest {
    pub name: String,
    pub aliases: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateGameRequest {
    pub name: Option<String>,
    pub aliases: Option<Vec<String>>,
}

// 响应 DTOs
#[derive(Debug, Serialize)]
pub struct GameResponse {
    pub id: i64,
    pub name: String,
    pub aliases: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

// 分页列表响应
#[derive(Debug, Serialize)]
pub struct GameListResponse {
    pub total: u64,
    pub games: Vec<GameResponse>,
}
```

## Constants

```rust
// 名称和别名最大长度
pub const MAX_NAME_LENGTH: usize = 50;
pub const MAX_ALIAS_LENGTH: usize = 50;

// 每个游戏最大别名数量
pub const MAX_ALIASES_COUNT: usize = 20;

// 默认分页大小
pub const DEFAULT_PAGE_SIZE: u64 = 10;

// 最大分页大小
pub const MAX_PAGE_SIZE: u64 = 100;
```

## Migrations

迁移文件位于 `crates/hiveweb/migrations/`，按版本号顺序应用。

### V045__create_games_table.sql

创建 `games` 表（见上方 DDL）。

### V046__create_game_alias_entries_table.sql

创建 `game_alias_entries` 表（见上方 DDL）。

## Indexes

### games 表
- `idx_games_name`: name 字段唯一索引（按名称查询和唯一性约束）

### game_alias_entries 表
- `idx_game_alias_entries_game_id`: game_id 索引（查询某游戏的所有别名）
- `idx_game_alias_entries_alias`: alias 字段唯一索引（全局唯一约束 + 别名搜索）

## SQL Patterns

### List with pagination + search
```sql
SELECT SQL_CALC_FOUND_ROWS
    g.id, g.name, g.created_at, g.updated_at,
    JSON_ARRAYAGG(gae.alias) AS aliases
FROM games g
LEFT JOIN game_alias_entries gae ON gae.game_id = g.id
WHERE g.name LIKE ? OR EXISTS (
    SELECT 1 FROM game_alias_entries
    WHERE game_id = g.id AND alias LIKE ?
)
GROUP BY g.id
ORDER BY g.id
LIMIT ? OFFSET ?;
```

### Count for pagination
```sql
SELECT COUNT(DISTINCT g.id)
FROM games g
WHERE g.name LIKE ? OR EXISTS (
    SELECT 1 FROM game_alias_entries
    WHERE game_id = g.id AND alias LIKE ?
);
```

### Create game + aliases (transaction)
```sql
-- Step 1: INSERT INTO games (name) VALUES (?)
-- Step 2: For each alias: INSERT INTO game_alias_entries (game_id, alias) VALUES (?, ?)
-- If any UNIQUE conflict occurs, rollback entire transaction
```

### Update game + aliases (transaction)
```sql
-- Step 1: UPDATE games SET name = ?, updated_at = NOW() WHERE id = ?
-- Step 2: DELETE FROM game_alias_entries WHERE game_id = ?
-- Step 3: For each new alias: INSERT INTO game_alias_entries (game_id, alias) VALUES (?, ?)
```
