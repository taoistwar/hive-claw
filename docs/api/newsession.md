# 创建新会话

```
POST /api/newsession?sign={md5}
```

为用户创建新的聊天会话。调用后该用户后续消息将关联到新 session。

## 鉴权

MD5 签名鉴权。

请求 URL 中需要附带 `sign` 查询参数，其值为：

```
MD5(ASSISTANT_SECRET + "/api/newsession?body=" + 请求体 JSON 字符串)
```

> 若环境变量 `ASSISTANT_SECRET` 为空，则跳过签名校验。

## 请求头

| Header | 值 |
|--------|-----|
| `Content-Type` | `application/json; charset=UTF-8` |

## 请求体

```json
{
  "user_id": 12345
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `user_id` | `i64` | 是 | 用户 ID，必须大于 0 |

## 处理流程

1. MD5 签名校验
2. 校验 `user_id` > 0
3. 查询外部 DB 确认用户存在
4. 同步用户信息到本地 DB（uid / nickname）
5. 创建新 `chat_sessions_user` 记录

## 示例请求

```bash
BODY='{"user_id":12345}'
SIGN=$(echo -n "${ASSISTANT_SECRET}/api/newsession?body=${BODY}" | md5sum | awk '{print $1}')

curl -X POST "http://localhost:3300/api/newsession?sign=${SIGN}" \
  -H "Content-Type: application/json; charset=UTF-8" \
  -d "${BODY}"
```

## 成功响应 `200 OK`

```json
{
  "success": true
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `success` | `bool` | 固定为 `true` |

## 错误响应

| 状态码 | 说明 |
|--------|------|
| `400` | 参数校验失败（`user_id` ≤ 0、MD5 签名校验失败、请求体格式错误、Content-Type 不正确） |
| `400` | 外部用户不存在 |
| `500` | 内部错误（外部 DB 不可用、用户数据查询失败、用户同步失败、会话创建失败） |
