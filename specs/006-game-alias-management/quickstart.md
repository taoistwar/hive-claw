# Quick Start: Game Alias Management

> **已废弃（Legacy，2026-07-14）：** 本规格描述的游戏别名管理前后端已停止业务使用，仅为兼容历史数据、接口和审计记录保留。不得以本文档为依据新增调用、部署或功能扩展；下文仅作为历史设计档案。

**Created**: 2026-06-01  
**Feature**: Game Alias Management (006-game-alias-management)

## Prerequisites

This feature is a sub-module of the admin center (003-admin-center). It assumes:

1. The admin center backend (`crates/hiveweb`) and frontend (`web-admin/`) are already set up
2. MySQL 8.0+ is running with the admin center database
3. The authentication middleware and RBAC system from 003-admin-center are functional

## Setup Steps

### 1. Apply Database Migrations

```bash
# Run migrations (includes V045__create_games_table.sql and V046__create_game_alias_entries_table.sql)
cargo run --bin migrate
```

### 2. Backend Setup

The game alias management code extends the existing hiveweb crate:

```bash
cd /home/developer/agent/hive-claw
cargo build -p hiveweb
```

### 3. Frontend Setup

```bash
cd web-admin
npm install
npm run dev
```

### 4. Access the Feature

1. Log in as a System or Super admin
2. Navigate to "游戏别名管理" in the sidebar menu
3. For Normal admins: view-only access

## API Testing Examples

### List Game Aliases

```bash
curl -H "Authorization: Bearer <token>" \
  "http://localhost:3300/api/game-aliases?page=1&page_size=10"
```

### Search Game Aliases

```bash
curl -H "Authorization: Bearer <token>" \
  "http://localhost:3300/api/game-aliases?q=王者"
```

### Create Game Alias

```bash
curl -X POST -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"name":"王者荣耀","aliases":["Honor of Kings","王者农药","WZRY"]}' \
  "http://localhost:3300/api/game-aliases"
```

### Update Game Alias

```bash
curl -X PUT -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"name":"王者荣耀（国服）","aliases":["Honor of Kings","王者农药"]}' \
  "http://localhost:3300/api/game-aliases/1"
```

### Delete Game Alias

```bash
curl -X DELETE -H "Authorization: Bearer <token>" \
  "http://localhost:3300/api/game-aliases/1"
```

### Permission Test (Normal Admin)

```bash
# Normal admin should get 403 for write operations
curl -X POST -H "Authorization: Bearer <normal-token>" \
  -H "Content-Type: application/json" \
  -d '{"name":"Test","aliases":["test1"]}' \
  "http://localhost:3300/api/game-aliases"

# Expected response:
# {"code": 2001, "data": null, "message": "权限不足，无法添加游戏别名"}
```

### Alias Uniqueness Test

```bash
# If "Honor of Kings" is already used by another game:
curl -X POST -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"name":"Test Game","aliases":["Honor of Kings"]}' \
  "http://localhost:3300/api/game-aliases"

# Expected response:
# {"code": 4008, "data": null, "message": "别名已被其他游戏使用"}
```
