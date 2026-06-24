# API Contract: 对外 Assistant API

**Version**: 1.0  
**Endpoint**: `POST /api/assistant`  
**Authentication**: MD5 签名鉴权（预共享 secret）

## Request

### Headers

```
Content-Type: application/json; charset=UTF-8
```

### Body

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `user_id` | integer (>0) | Yes | 外部系统中的用户 ID |
| `message` | string (非空) | Yes | 用户输入的消息内容 |

### Signature（Query Parameter）

```
POST /api/assistant?sign={md5_signature}
```

签名算法：
```
sign = MD5(ASSISTANT_SECRET + "/api/assistant" + "?body=" + requestBody)
```

其中 `requestBody` 为原始 JSON 字符串（不含空格，保持紧凑格式）。

#### 签名示例

```
secret = "abc123"
requestBody = '{"user_id":123,"message":"你好"}'
signString = 'abc123/api/assistant?body={"user_id":123,"message":"你好"}'
sign = md5(signString)
```

### Constraints

| Constraint | Value |
|------------|-------|
| `user_id` 必须 | 正整数 (>0) |
| `message` 必须 | 非空字符串 |
| Content-Type 必须 | `application/json; charset=UTF-8` |

## Response

### Success (200 OK)

```json
{
  "code": 0,
  "message": "success",
  "data": {
    "success": true,
    "reply": "AI 回复内容...",
    "message": "ok"
  }
}
```

### Business Errors (200 OK, `data.success = false`)

| Scenario | `data.message` |
|----------|---------------|
| `user_id` 不存在于 `cloud_user` | `"User not found"` |
| 已达每日限额 | `"Daily limit reached (50/50)"` |
| 外部 DB 未配置 | `"Assistant service unavailable"` |
| Redis 不可用 | `"Redis unavailable"` |

### Validation Errors (200 OK or 400, `data.success = false`)

| Scenario | `data.message` |
|----------|---------------|
| `user_id ≤ 0` | `"user_id must be positive"` |
| `message` 为空或仅含空白 | `"message must not be empty"` |
| 签名不匹配或缺失 | `"Invalid signature"` |
| JSON 解析失败 | `"Invalid request body"` |
| Content-Type 不匹配 | `"Invalid Content-Type, must be: application/json; charset=UTF-8"` |

### System Errors (500)

| Scenario | Behavior |
|----------|----------|
| LLM 调用超时/失败 | 返回 `"Service busy, please retry later"`，不消耗配额 |
| 内部 DB 错误 | `{"code": 5000, "message": "Service unavailable"}` |

## Processing Flow

```
Request
  │
  ├─ 1. Content-Type 校验 ──→ 不匹配 → 400
  ├─ 2. JSON 解析 ──→ 失败 → 400
  ├─ 3. MD5 签名校验 ──→ 不匹配 → 403
  ├─ 4. user_id <= 0 ──→ 是 → 400 (不消耗配额)
  ├─ 5. message 为空 ──→ 是 → 400 (不消耗配额)
  │
  ├─ 6. 外部 DB cloud_user 查询 ──→ 不存在 → User not found (不消耗配额)
  │
  ├─ 7. 外部 DB cc_user_membership 查询 ──→ VIP/普通 判定
  ├─ 8. 全局配置读取 (vip_ask_times / normal_ask_times)
  ├─ 9. Redis INCR + 限额检查 ──→ 超限 → Daily limit reached
  │
  ├─ 10. 内部 users 表同步 (不存在 → INSERT)
  │
  ├─ 11. Agent 编排处理（Main Agent）──→ 成功 → 返回结果
  │              ──→ 失败 → DECR 配额 + 返回 busy 提示
  │
  └─ Response
```

## Daily Rate Limits

| 用户类型 | 配置 Key | 默认值 | Redis Key |
|----------|----------|--------|-----------|
| VIP 会员 | `vip_ask_times` (GlobalConfig) | 50 | `assistant:daily:{user_id}` |
| 普通用户 | `normal_ask_times` (GlobalConfig) | 5 | `assistant:daily:{user_id}` |

- 计数器在首次当天访问时创建，TTL 为到次日 00:00:00 的秒数
- 以下场景不消耗配额：用户不存在、签名失败、参数校验失败、LLM 调用失败

## VIP Membership Rules

- 查询 `cc_user_membership` 表，`id = user_id`
- `effective_end_time >= 当前时间` → 有效 VIP（边界包含，"当天全天有效"）
- 无记录或无 `effective_end_time` 但有记录 → 永久有效 VIP
- 无记录 → 普通用户
