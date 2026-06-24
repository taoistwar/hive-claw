# API Contracts: Game Alias Management

**Created**: 2026-06-01  
**Feature**: Game Alias Management (006-game-alias-management)

## Overview

本文档定义游戏别名管理功能的 RESTful API 契约。复用管理中心的认证体系、响应格式和错误处理机制。

## Base URL

- Development: `http://localhost:3300/api`
- Production: `https://admin.example.com/api`

## Authentication

所有游戏别名 API 都需要在请求头中携带 JWT Token:

```
Authorization: Bearer <access_token>
```

## Permission Model

| 角色 | 查看列表 | 添加 | 编辑 | 删除 |
|------|---------|------|------|------|
| Normal (1) | 允许 | 拒绝 (403) | 拒绝 (403) | 拒绝 (403) |
| System (2) | 允许 | 允许 | 允许 | 允许 |
| Super (3) | 允许 | 允许 | 允许 | 允许 |

## Response Format

复用管理中心的统一响应格式：

### Success Response

```json
{
  "code": 0,
  "data": { ... },
  "message": "操作成功"
}
```

### Error Response

```json
{
  "code": 4001,
  "data": null,
  "message": "具体错误描述"
}
```

## Error Codes (4001-4999: Game Alias Management)

```
4001: GAME_ALIAS_NOT_FOUND          // 游戏别名不存在
4002: GAME_NAME_ALREADY_EXISTS      // 游戏名称已存在
4003: GAME_NAME_EMPTY               // 游戏名称为空
4004: GAME_NAME_TOO_LONG            // 游戏名称超过最大长度 (50)
4005: ALIASES_EMPTY                 // 别名数组为空
4006: ALIAS_TOO_LONG                // 别名超过最大长度 (50)
4007: ALIASES_TOO_MANY              // 别名数量超过最大限制 (20)
4008: ALIAS_ALREADY_IN_USE          // 别名已被其他游戏使用（跨游戏唯一）
4009: INSUFFICIENT_PERMISSION       // 权限不足 (复用 2001)
5000: INTERNAL_ERROR                // 内部错误
5001: DATABASE_ERROR                // 数据库错误
5002: VALIDATION_ERROR              // 参数验证失败
```

## API Endpoints

### 1. 游戏别名管理 API

#### 1.1 获取游戏别名列表

**Endpoint**: `GET /api/game-aliases`

**Description**: 获取游戏别名列表，支持搜索和分页

**Permission**: All authenticated admins (Normal, System, Super)

**Request Headers**:
```
Authorization: Bearer <access_token>
```

**Query Parameters**:
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| page | integer | 1 | 页码（从 1 开始） |
| page_size | integer | 10 | 每页数量（最大 100） |
| q | string | - | 搜索关键词（匹配游戏名称或别名） |

**Success Response** (200 OK):
```json
{
  "code": 0,
  "data": {
    "total": 25,
    "game_aliases": [
      {
        "id": 1,
        "name": "王者荣耀",
        "aliases": ["Honor of Kings", "王者农药", "WZRY"],
        "created_at": "2026-06-01T10:00:00Z",
        "updated_at": "2026-06-01T10:00:00Z"
      },
      {
        "id": 2,
        "name": "英雄联盟",
        "aliases": ["LOL", "LOL"],
        "created_at": "2026-06-01T11:00:00Z",
        "updated_at": "2026-06-01T11:00:00Z"
      }
    ]
  },
  "message": "获取成功"
}
```

**Search Behavior**:
- `search` 参数对游戏名称和别名进行模糊匹配
- 匹配条件：`name LIKE '%search%' OR aliases 中包含 '%search%'`

---

#### 1.2 添加游戏别名

**Endpoint**: `POST /api/game-aliases`

**Description**: 添加新的游戏别名记录

**Permission**: System (2) and Super (3) only. Returns 403 for Normal (1).

**Request Headers**:
```
Authorization: Bearer <access_token>
Content-Type: application/json
```

**Request Body**:
```json
{
  "name": "王者荣耀",
  "aliases": ["Honor of Kings", "王者农药", "WZRY"]
}
```

**Validation Rules**:
- `name`: 必填，1-50 字符，唯一
- `aliases`: 必填，非空数组，1-20 个元素，每个元素 1-50 字符，前后端双重去重（前端提交前去重，后端存储前再次去重）
- 别名全局唯一：不能与任何已有游戏使用的别名重复

**Success Response** (201 Created):
```json
{
  "code": 0,
  "data": {
    "id": 3,
    "name": "王者荣耀",
    "aliases": ["Honor of Kings", "王者农药", "WZRY"],
    "created_at": "2026-06-01T14:00:00Z",
    "updated_at": "2026-06-01T14:00:00Z"
  },
  "message": "添加成功"
}
```

**Error Responses**:

```json
// 游戏名称已存在
{
  "code": 4002,
  "data": null,
  "message": "游戏名称已存在"
}
```

```json
// 权限不足
{
  "code": 2001,
  "data": null,
  "message": "权限不足，无法添加游戏别名"
}
```

```json
// 别名数组为空
{
  "code": 4005,
  "data": null,
  "message": "别名数组不能为空"
}
```

```json
// 别名数量过多
{
  "code": 4007,
  "data": null,
  "message": "别名数量不能超过 20 个"
}
```

```json
// 别名已被其他游戏使用
{
  "code": 4008,
  "data": null,
  "message": "别名已被其他游戏使用"
}
```

---

#### 1.3 获取游戏别名详情

**Endpoint**: `GET /api/game-aliases/:id`

**Description**: 获取指定游戏别名的详细信息

**Permission**: All authenticated admins

**Request Headers**:
```
Authorization: Bearer <access_token>
```

**Path Parameters**:
| Parameter | Type | Description |
|-----------|------|-------------|
| id | integer | 游戏别名 ID |

**Success Response** (200 OK):
```json
{
  "code": 0,
  "data": {
    "id": 1,
    "name": "王者荣耀",
    "aliases": ["Honor of Kings", "王者农药", "WZRY"],
    "created_at": "2026-06-01T10:00:00Z",
    "updated_at": "2026-06-01T10:00:00Z"
  },
  "message": "获取成功"
}
```

**Error Response**:

```json
// 游戏别名不存在
{
  "code": 4001,
  "data": null,
  "message": "游戏别名不存在"
}
```

---

#### 1.4 编辑游戏别名

**Endpoint**: `PUT /api/game-aliases/:id`

**Description**: 编辑游戏别名信息（名称或别名）

**Permission**: System (2) and Super (3) only. Returns 403 for Normal (1).

**Request Headers**:
```
Authorization: Bearer <access_token>
Content-Type: application/json
```

**Path Parameters**:
| Parameter | Type | Description |
|-----------|------|-------------|
| id | integer | 游戏别名 ID |

**Request Body** (所有字段可选，但至少提供一个):
```json
{
  "name": "王者荣耀（国服）",
  "aliases": ["Honor of Kings", "王者农药"]
}
```

**Validation Rules**:
- `name`: 可选，1-50 字符，唯一
- `aliases`: 可选，非空数组，1-20 个元素，每个元素 1-50 字符，前后端双重去重
- 别名全局唯一：不能与任何已有游戏使用的别名重复
- 至少提供 name 或 aliases 之一

**Success Response** (200 OK):
```json
{
  "code": 0,
  "data": {
    "id": 1,
    "name": "王者荣耀（国服）",
    "aliases": ["Honor of Kings", "王者农药"],
    "created_at": "2026-06-01T10:00:00Z",
    "updated_at": "2026-06-01T15:00:00Z"
  },
  "message": "修改成功"
}
```

**Error Responses**:

```json
// 游戏名称已存在（修改名称时与已有记录冲突）
{
  "code": 4002,
  "data": null,
  "message": "游戏名称已存在"
}
```

```json
// 权限不足
{
  "code": 2001,
  "data": null,
  "message": "权限不足，无法编辑游戏别名"
}
```

```json
// 别名已被其他游戏使用
{
  "code": 4008,
  "data": null,
  "message": "别名已被其他游戏使用"
}
```

---

#### 1.5 删除游戏别名

**Endpoint**: `DELETE /api/game-aliases/:id`

**Description**: 删除游戏别名记录

**Permission**: System (2) and Super (3) only. Returns 403 for Normal (1).

**Request Headers**:
```
Authorization: Bearer <access_token>
```

**Path Parameters**:
| Parameter | Type | Description |
|-----------|------|-------------|
| id | integer | 游戏别名 ID |

**Success Response** (200 OK):
```json
{
  "code": 0,
  "data": null,
  "message": "删除成功"
}
```

**Error Responses**:

```json
// 游戏别名不存在
{
  "code": 4001,
  "data": null,
  "message": "游戏别名不存在"
}
```

```json
// 权限不足
{
  "code": 2001,
  "data": null,
  "message": "权限不足，无法删除游戏别名"
}
```
