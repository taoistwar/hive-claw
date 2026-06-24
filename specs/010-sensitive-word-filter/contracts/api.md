# API Contracts: 敏感词过滤

**Feature**: 010-sensitive-word-filter | **Date**: 2026-06-07

## 1. 敏感词输入拦截

当 `/api/assistant` 收到包含敏感词的消息时，返回拦截响应。

### Request (unchanged)

```http
POST /api/assistant?sign={md5}
Content-Type: application/json; charset=UTF-8

{
    "user_id": 123,
    "message": "包含敏感词的文本",
    "channel": "app",
    "platform": "android",
    "app_version": "1.0.0"
}
```

### Response (intercepted — NEW)

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
    "reply": "抱歉，您的请求暂时无法处理，请稍后再试。",
    "filtered": true
}
```

### Response (normal — unchanged)

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
    "id": 456,
    "session_id": 789,
    "role": "assistant",
    "content": "正常的 AI 回复内容",
    ...
}
```

## 2. Agent 输出替换

当 Agent 生成包含敏感词的回复时，正常响应中的 `content` 字段被替换为友好提示。

### Response (output replaced — NEW)

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
    "id": 456,
    "session_id": 789,
    "role": "assistant",
    "content": "抱歉，系统无法处理您的请求，请稍后重试。",
    "filtered": true,
    ...
}
```

## 3. 敏感词 CRUD 管理 API

所有管理 API 需要 admin 认证（复用现有 `admin_auth_middleware`）。

### 3.1 列表查询

```http
GET /api/sensitive-words?page=1&page_size=20&search=关键词&enabled=true
Authorization: Bearer {admin_token}

HTTP/1.1 200 OK
{
    "total": 100,
    "words": [
        {
            "id": 1,
            "word": "敏感词1",
            "match_mode": "exact",
            "enabled": true,
            "created_at": "2026-06-07T12:00:00Z",
            "updated_at": "2026-06-07T12:00:00Z"
        }
    ]
}
```

### 3.2 创建

```http
POST /api/sensitive-words
Authorization: Bearer {admin_token}
Content-Type: application/json

{
    "word": "敏感词1",
    "match_mode": "exact"
}

HTTP/1.1 200 OK
{
    "id": 1,
    "word": "敏感词1",
    "match_mode": "exact",
    "enabled": true,
    "created_at": "2026-06-07T12:00:00Z",
    "updated_at": "2026-06-07T12:00:00Z"
}
```

**Validation**:
- `word`: required, non-empty, ≤ 512 chars
- `match_mode`: required, must be `"exact"` or `"regex"`
- If `match_mode = "regex"`, `word` must be a valid regex (RFC regex syntax)
- Duplicate `(word, match_mode)` rejected: HTTP 409

### 3.3 更新

```http
PUT /api/sensitive-words/:id
Authorization: Bearer {admin_token}
Content-Type: application/json

{
    "word": "修改后的词",
    "match_mode": "exact",
    "enabled": true
}

HTTP/1.1 200 OK
{ ... updated row ... }
```

**Validation**: Same as create. Updates auto-trigger cache refresh.

### 3.4 删除

```http
DELETE /api/sensitive-words/:id
Authorization: Bearer {admin_token}

HTTP/1.1 200 OK
{ "deleted": true }
```

## Error Responses

| Scenario | HTTP Status | Body |
|----------|-------------|------|
| Invalid regex in word | 400 | `{"error": "Invalid regex: ..."}` |
| Duplicate (word, mode) | 409 | `{"error": "Sensitive word already exists"}` |
| Word not found (update/delete) | 404 | `{"error": "Sensitive word not found"}` |
| Unauthorized | 401 | `{"error": "Unauthorized"}` |
