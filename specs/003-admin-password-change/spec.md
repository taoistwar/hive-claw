# Feature Specification: 管理员修改密码

**Feature Branch**: `003-admin-center`
**Created**: 2026-05-31
**Status**: Draft
**Input**: 添加管理员修改密码功能

## User Scenarios & Testing *(mandatory)*

### User Story 1 - 管理员修改自己的密码 (Priority: P1)

管理员登录后可以修改自己的登录密码。修改密码时需要验证旧密码，并输入新密码两次进行确认。修改成功后，当前会话保持有效，但下次登录需要使用新密码。

**Why this priority**: 密码修改是管理系统的基础安全功能，管理员需要定期更换密码以保证账户安全。

**Independent Test**: 可以独立测试密码修改流程，包括旧密码验证、新密码验证、密码更新。

**Acceptance Scenarios**:

1. **Given** 管理员已登录并打开修改密码页面，**When** 管理员输入正确的旧密码和符合要求的新密码并点击确认，**Then** 系统提示"密码修改成功"，下次登录需使用新密码
2. **Given** 管理员正在修改密码，**When** 管理员输入的旧密码不正确，**Then** 系统提示"旧密码错误"
3. **Given** 管理员正在修改密码，**When** 管理员输入的新密码不符合要求（长度不足、包含非法字符等），**Then** 系统提示具体的密码格式要求
4. **Given** 管理员正在修改密码，**When** 两次输入的新密码不一致，**Then** 系统提示"两次输入的新密码不一致"
5. **Given** 管理员已修改密码，**When** 管理员使用新密码登录，**Then** 系统成功登录
6. **Given** 管理员已修改密码，**When** 管理员使用旧密码登录，**Then** 系统提示"密码错误"

---

### Edge Cases

- 修改密码后，当前 Token 保持有效（不强制重新登录）
- 新密码不能与旧密码相同
- 密码修改操作需要记录审计日志
- 连续多次旧密码错误也应触发锁定机制（与登录失败共用锁定策略）

## Requirements *(mandatory)*

### Functional Requirements

- **FR-101**: 系统必须支持管理员修改自己的密码
- **FR-102**: 系统必须验证旧密码的正确性
- **FR-103**: 系统必须验证新密码符合要求（6-20 位，字母+数字）
- **FR-104**: 系统必须验证两次输入的新密码一致
- **FR-105**: 系统必须验证新密码与旧密码不同
- **FR-106**: 系统必须在密码修改成功后写入审计日志
- **FR-107**: 系统必须对旧密码错误进行失败计数，连续 5 次错误后锁定 15 分钟
- **FR-108**: 系统必须在前端提供密码强度提示

### API Contract

#### 修改密码

**Endpoint**: `POST /api/auth/change-password`

**Request Headers**:
```
Authorization: Bearer <access_token>
Content-Type: application/json
```

**Request Body**:
```json
{
  "old_password": "old_password_123",
  "new_password": "new_password_456"
}
```

**Validation Rules**:
- `old_password`: 必填，6-20 位
- `new_password`: 必填，6-20 位，字母+数字组合

**Success Response** (200 OK):
```json
{
  "code": 0,
  "data": null,
  "message": "密码修改成功"
}
```

**Error Responses**:

```json
// 旧密码错误
{
  "code": 1001,
  "data": null,
  "message": "旧密码错误"
}
```

```json
// 新密码与旧密码相同
{
  "code": 3008,
  "data": null,
  "message": "新密码不能与旧密码相同"
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

### Error Code Addition

| Code | Description |
|------|-------------|
| 3008 | NEW_PASSWORD_SAME_AS_OLD // 新密码不能与旧密码相同 |

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-101**: 管理员可以在 30 秒内完成密码修改流程
- **SC-102**: 密码修改成功率 100%（正确输入时）
- **SC-103**: 旧密码错误时，100% 返回明确错误提示
- **SC-104**: 密码修改操作 100% 记录审计日志

## Assumptions

- 修改密码需要管理员已登录（需要 JWT Token）
- 修改密码后，当前会话保持有效
- 新密码要求与注册时相同（6-20 位，字母+数字）
- 密码修改失败计入锁定计数器（与登录失败共用）
