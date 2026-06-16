# 获取用户历史消息

```
POST /api/messages?sign={md5}
```

获取指定时间之前的最近 10 条用户聊天记录。

## 鉴权

MD5 签名鉴权。

请求 URL 中需要附带 `sign` 查询参数，其值为：

```
MD5(ASSISTANT_SECRET + "/api/messages?body=" + json_body)
```

> 若环境变量 `ASSISTANT_SECRET` 为空，则跳过签名校验。

## 请求头

| Header | 值 |
|--------|-----|
| `Content-Type` | `application/json; charset=UTF-8` |

## 请求体

```json
{
  "user_id": 12345,
  "date": "2026-06-03 14:30:00"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `user_id` | `i64` | 是 | 用户 ID，必须大于 0 |
| `date` | `String` | 是 | 最后一条聊天记录的时间，格式 `YYYY-MM-DD HH:MM:SS`，返回该时间之前的最近 10 条 |

## URL 查询参数

| 参数 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `sign` | `String` | 否* | MD5 签名（`ASSISTANT_SECRET` 非空时必填） |

## 示例请求

```bash
USER_ID=12345
DATE="2026-06-03 14:30:00"
BODY="{\"user_id\":${USER_ID},\"date\":\"${DATE}\"}"
SIGN_STR="/api/messages?body=${BODY}"
SIGN=$(echo -n "${ASSISTANT_SECRET}${SIGN_STR}" | md5sum | awk '{print $1}')

curl -X POST "http://localhost:3300/api/messages?sign=${SIGN}" \
  -H "Content-Type: application/json; charset=UTF-8" \
  -d "${BODY}"
```

## 成功响应 `200 OK`

```json
{
  "messages": [
    {
      "id": 100,
      "session_id": 5,
      "user_id": 12345,
      "role": "assistant",
      "content": "您好！以下是为您推荐的游戏...",
      "elapsed_ms": 1523,
      "extensions": [{
        "content_type": "card",
        "payload": {
          "type": "goPay",
          "info": {
            "effective_end_time": "",
            "membership_category": "",
            "level_name": "",
            "total_coins": 900000.0,
            "expire_coins_7d": 5000.0
          }
        }
      }],
      "created_at": "2026-06-02T14:30:00Z"
    },
    {
      "id": 99,
      "session_id": 5,
      "user_id": 12345,
      "role": "user",
      "content": "推荐一款游戏",
      "elapsed_ms": null,
      "extension": null,
      "extensions": null,
      "created_at": "2026-06-02T14:29:55Z"
    }
  ]
}
```

### 响应字段说明

| 字段 | 类型 | 说明 |
|------|------|------|
| `messages` | `Array` | 消息数组（最多 10 条），按时间倒序排列 |
| `messages[].id` | `i64` | 消息唯一 ID |
| `messages[].session_id` | `i64` | 所属会话 ID |
| `messages[].user_id` | `i64` | 用户 ID |
| `messages[].role` | `String` | 角色：`user` / `assistant` / `tool` / `system` |
| `messages[].content` | `Option<String>` | 消息文本内容 |
| `messages[].elapsed_ms` | `Option<i32>` | assistant 消息的处理耗时（毫秒），user 消息为 null |
| `messages[].extensions` | `Option<Vec<Value>>` | 扩展对象数组（≥ 2 条时出现），结构与 `/api/assistant` 一致 |
| `messages[].created_at` | `DateTime` | 消息创建时间（UTC） |

## 错误响应

| 状态码 | 说明 |
|--------|------|
| `400` | 参数校验失败（`user_id` ≤ 0、`date` 为空或格式错误） |
| `400` | MD5 签名校验失败 |
| `500` | 数据库查询失败 |
