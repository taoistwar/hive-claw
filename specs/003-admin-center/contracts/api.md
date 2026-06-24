# API Contracts: 管理中心

**Created**: 2026-05-25
**Feature**: 管理中心 (003-admin-center)

## Overview

本文档定义了管理中心系统的 RESTful API 契约。所有 API 遵循统一的响应格式和错误处理机制。

## Base URL

- Development: `http://localhost:3300/api`
- Production: `https://admin.example.com/api`

## Authentication

除登录接口外，所有 API 都需要在请求头中携带 JWT Token:

```
Authorization: Bearer <access_token>
```

## Response Format

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
  "code": 1001,
  "data": null,
  "message": "密码错误"
}
```

## Error Codes

| Code Range | Category | Description |
|------------|----------|-------------|
| 0 | Success | 操作成功 |
| 1001-1999 | Authentication | 认证相关错误 |
| 2001-2999 | Authorization | 权限相关错误 |
| 3001-3999 | Admin Management | 管理员管理错误 |
| 5000-5999 | System | 系统错误 |

### Detailed Error Codes

```rust
// 认证错误 (1001-1999)
1001: WRONG_PASSWORD          // 密码错误
1002: ACCOUNT_DISABLED        // 账户已被禁用
1003: ACCOUNT_LOCKED          // 账户已被锁定
1004: INVALID_TOKEN           // Token 无效或过期
1005: TOKEN_EXPIRED           // Token 已过期
1006: MISSING_TOKEN           // 缺少 Token
1007: REFRESH_TOKEN_EXPIRED   // 刷新 Token 已过期

// 权限错误 (2001-2999)
2001: INSUFFICIENT_PERMISSION // 权限不足
2002: OPERATION_NOT_ALLOWED   // 操作不被允许

// 管理员管理错误 (3001-3999)
3001: ADMIN_NOT_FOUND         // 管理员不存在
3002: PHONE_ALREADY_EXISTS    // 手机号已存在
3003: CANNOT_DELETE_SUPER     // 不能删除超级管理员
3004: CANNOT_DISABLE_LAST_SUPER // 不能禁用最后一个超级管理员
3005: INVALID_PHONE_FORMAT    // 手机号格式不正确
3006: INVALID_PASSWORD_LENGTH // 密码长度不符合要求
3007: INVALID_NICKNAME_LENGTH // 昵称长度不符合要求

// 系统错误 (5000-5999)
5000: INTERNAL_ERROR          // 内部错误
5001: DATABASE_ERROR          // 数据库错误
5002: VALIDATION_ERROR        // 参数验证失败
```

## API Endpoints

### 1. 认证 API

#### 1.1 管理员登录

**Endpoint**: `POST /api/auth/login`

**Description**: 管理员使用手机号和密码登录系统

**Request Headers**:
```
Content-Type: application/json
```

**Request Body**:
```json
{
  "phone": "18810154696",
  "password": "admin123"
}
```

**Validation Rules**:
- `phone`: 必填，11 位数字，中国大陆手机号格式
- `password`: 必填，6-20 位

**Success Response** (200 OK):
```json
{
  "code": 0,
  "data": {
    "token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxIiwicm9sZSI6MywiaWF0IjoxNjE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c",
    "refresh_token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxIiwiaWF0IjoxNjE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c",
    "admin": {
      "id": 1,
      "phone": "18810154696",
      "nickname": "Super Admin",
      "role": 3,
      "status": 1,
      "created_at": "2026-05-25T10:00:00Z",
      "last_login_at": "2026-05-25T12:00:00Z"
    }
  },
  "message": "登录成功"
}
```

**Error Responses**:

```json
// 密码错误
{
  "code": 1001,
  "data": null,
  "message": "密码错误"
}
```

```json
// 账户已被禁用
{
  "code": 1002,
  "data": null,
  "message": "账户已被禁用，请联系系统管理员"
}
```

```json
// 账户已被锁定
{
  "code": 1003,
  "data": null,
  "message": "账户已被临时锁定，请 15 分钟后重试"
}
```

```json
// 参数验证失败
{
  "code": 5002,
  "data": null,
  "message": "请填写完整的登录信息"
}
```

---

#### 1.2 管理员登出

**Endpoint**: `POST /api/auth/logout`

**Description**: 管理员登出系统，使当前 Token 失效

**Request Headers**:
```
Authorization: Bearer <access_token>
```

**Success Response** (200 OK):
```json
{
  "code": 0,
  "data": null,
  "message": "登出成功"
}
```

---

#### 1.3 获取当前管理员信息

**Endpoint**: `GET /api/auth/me`

**Description**: 获取当前登录管理员的详细信息

**Request Headers**:
```
Authorization: Bearer <access_token>
```

**Success Response** (200 OK):
```json
{
  "code": 0,
  "data": {
    "id": 1,
    "phone": "18810154696",
    "nickname": "Super Admin",
    "role": 3,
    "status": 1
  },
  "message": "获取成功"
}
```

---

### 2. 管理员管理 API

#### 2.1 获取管理员列表

**Endpoint**: `GET /api/admins`

**Description**: 获取所有管理员列表，支持分页

**Request Headers**:
```
Authorization: Bearer <access_token>
```

**Query Parameters**:
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| page | integer | 1 | 页码（从 1 开始） |
| page_size | integer | 10 | 每页数量（最大 100） |

**Success Response** (200 OK):
```json
{
  "code": 0,
  "data": {
    "total": 25,
    "admins": [
      {
        "id": 1,
        "phone": "18810154696",
        "nickname": "Super Admin",
        "role": 3,
        "status": 1,
        "created_at": "2026-05-25T10:00:00Z",
        "last_login_at": "2026-05-25T12:00:00Z"
      },
      {
        "id": 2,
        "phone": "13900139000",
        "nickname": "System Admin",
        "role": 2,
        "status": 1,
        "created_at": "2026-05-25T11:00:00Z",
        "last_login_at": "2026-05-25T11:30:00Z"
      }
    ]
  },
  "message": "获取成功"
}
```

---

#### 2.2 添加管理员

**Endpoint**: `POST /api/admins`

**Description**: 添加新的管理员账号

**Request Headers**:
```
Authorization: Bearer <access_token>
Content-Type: application/json
```

**Request Body**:
```json
{
  "phone": "13900139000",
  "nickname": "New Admin",
  "password": "password123",
  "role": 1
}
```

**Validation Rules**:
- `phone`: 必填，11 位数字，唯一
- `nickname`: 必填，2-20 字符
- `password`: 必填，6-20 位
- `role`: 必填，1=普通管理员，2=系统管理员，3=超级管理员

**Success Response** (201 Created):
```json
{
  "code": 0,
  "data": {
    "id": 3,
    "phone": "13900139000",
    "nickname": "New Admin",
    "role": 1,
    "status": 1,
    "created_at": "2026-05-25T14:00:00Z"
  },
  "message": "添加成功"
}
```

**Error Responses**:

```json
// 手机号已存在
{
  "code": 3002,
  "data": null,
  "message": "该手机号已被注册"
}
```

```json
// 权限不足
{
  "code": 2001,
  "data": null,
  "message": "权限不足，无法添加管理员"
}
```

---

#### 2.3 获取管理员详情

**Endpoint**: `GET /api/admins/:id`

**Description**: 获取指定管理员的详细信息

**Request Headers**:
```
Authorization: Bearer <access_token>
```

**Path Parameters**:
| Parameter | Type | Description |
|-----------|------|-------------|
| id | integer | 管理员 ID |

**Success Response** (200 OK):
```json
{
  "code": 0,
  "data": {
    "id": 1,
    "phone": "18810154696",
    "nickname": "Super Admin",
    "role": 3,
    "status": 1,
    "created_at": "2026-05-25T10:00:00Z",
    "last_login_at": "2026-05-25T12:00:00Z"
  },
  "message": "获取成功"
}
```

**Error Response**:

```json
// 管理员不存在
{
  "code": 3001,
  "data": null,
  "message": "管理员不存在"
}
```

---

#### 2.4 修改管理员

**Endpoint**: `PUT /api/admins/:id`

**Description**: 修改管理员信息（ID 和注册时间不可修改）

**Request Headers**:
```
Authorization: Bearer <access_token>
Content-Type: application/json
```

**Path Parameters**:
| Parameter | Type | Description |
|-----------|------|-------------|
| id | integer | 管理员 ID |

**Request Body** (所有字段可选):
```json
{
  "nickname": "Updated Nickname",
  "phone": "13900139000",
  "status": 1
}
```

**Validation Rules**:
- `nickname`: 可选，2-20 字符
- `phone`: 可选，11 位数字，唯一
- `status`: 可选，0=禁用，1=启用

**Success Response** (200 OK):
```json
{
  "code": 0,
  "data": {
    "id": 1,
    "phone": "13900139000",
    "nickname": "Updated Nickname",
    "role": 3,
    "status": 1,
    "created_at": "2026-05-25T10:00:00Z",
    "last_login_at": "2026-05-25T12:00:00Z"
  },
  "message": "修改成功"
}
```

---

#### 2.5 删除管理员

**Endpoint**: `DELETE /api/admins/:id`

**Description**: 删除管理员账号（超级管理员不可删除）

**Request Headers**:
```
Authorization: Bearer <access_token>
```

**Path Parameters**:
| Parameter | Type | Description |
|-----------|------|-------------|
| id | integer | 管理员 ID |

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
// 不能删除超级管理员
{
  "code": 3003,
  "data": null,
  "message": "不能删除超级管理员"
}
```

```json
// 权限不足
{
  "code": 2001,
  "data": null,
  "message": "权限不足，无法删除管理员"
}
```

---

#### 2.6 禁用/启用管理员

**Endpoint**: `PATCH /api/admins/:id/status`

**Description**: 禁用或启用管理员账号

**Request Headers**:
```
Authorization: Bearer <access_token>
Content-Type: application/json
```

**Path Parameters**:
| Parameter | Type | Description |
|-----------|------|-------------|
| id | integer | 管理员 ID |

**Request Body**:
```json
{
  "status": 0  // 0=禁用，1=启用
}
```

**Success Response** (200 OK):
```json
{
  "code": 0,
  "data": null,
  "message": "操作成功"
}
```

**Error Responses**:

```json
// 不能禁用最后一个超级管理员
{
  "code": 3004,
  "data": null,
  "message": "不能禁用最后一个超级管理员"
}
```

---

### 3. 仪表盘 API

#### 3.1 获取统计数据

**Endpoint**: `GET /api/dashboard/stats`

**Description**: 获取系统概览统计数据

**Request Headers**:
```
Authorization: Bearer <access_token>
```

**Success Response** (200 OK):
```json
{
  "code": 0,
  "data": {
    "total_admins": 25,
    "online_admins": 5,
    "today_logins": 48
  },
  "message": "获取成功"
}
```

**Field Descriptions**:
- `total_admins`: 管理员总数
- `online_admins`: 当前在线管理员数（24 小时内有登录记录）
- `today_logins`: 今日登录次数

---

#### 3.2 获取最近登录记录

**Endpoint**: `GET /api/dashboard/recent-logins`

**Description**: 获取最近的登录记录（最近 10 条）

**Request Headers**:
```
Authorization: Bearer <access_token>
```

**Query Parameters**:
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| limit | integer | 10 | 返回数量（最大 50） |

**Success Response** (200 OK):
```json
{
  "code": 0,
  "data": [
    {
      "id": 100,
      "admin_id": 1,
      "admin_nickname": "Super Admin",
      "login_at": "2026-05-25T12:00:00Z",
      "ip_address": "192.168.1.100",
      "success": true,
      "failure_reason": null
    },
    {
      "id": 99,
      "admin_id": 2,
      "admin_nickname": "System Admin",
      "login_at": "2026-05-25T11:30:00Z",
      "ip_address": "192.168.1.101",
      "success": false,
      "failure_reason": "WRONG_PASSWORD"
    }
  ],
  "message": "获取成功"
}
```

---

## Rate Limiting

所有 API 接口都受到速率限制：

- 登录接口：同一手机号最多 5 次/分钟
- 其他接口：100 次/分钟/IP

**Rate Limit Response** (429 Too Many Requests):
```json
{
  "code": 429,
  "data": null,
  "message": "请求过于频繁，请稍后再试"
}
```

## CORS

开发环境允许跨域访问：

```
Access-Control-Allow-Origin: http://localhost:5173
Access-Control-Allow-Credentials: true
Access-Control-Allow-Methods: GET, POST, PUT, DELETE, PATCH, OPTIONS
Access-Control-Allow-Headers: Content-Type, Authorization
```

生产环境需配置具体的域名。

## Versioning

API 版本通过 URL 路径标识：

- Current: `/api/v1/...`
- 未来版本：`/api/v2/...`

当前实现使用 `/api/...` 默认为 v1 版本。
