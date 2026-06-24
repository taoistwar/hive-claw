# 执行推荐游戏

```
POST /api/recommended-games/execute?sign={md5}
```

点击推荐游戏后，自动记录两条聊天消息（用户消息 + assistant 游戏卡片）。

## 鉴权

MD5 签名鉴权。

请求 URL 中需要附带 `sign` 查询参数，其值为：

```
MD5(ASSISTANT_SECRET + "/api/recommended-games/execute" + "?body=" + 请求体 JSON 字符串)
```

> 若环境变量 `ASSISTANT_SECRET` 为空，则跳过签名校验。

## 请求头

| Header | 值 |
|--------|-----|
| `Content-Type` | `application/json; charset=UTF-8` |

## 请求体

```json
{
  "user_id": "12345",
  "game_id": "game_001",
  "channel": "app",
  "client_type": "android",
  "client_version": "1.0.0"
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `user_id` | `String` | 是 | 用户 ID，不能为空 |
| `game_id` | `String` | 是 | 推荐游戏的 game_id，不能为空 |
| `channel` | `String` | 否 | 渠道标识（如 `app`、`web`） |
| `client_type` | `String` | 否 | 客户端类型（如 `android`、`iphone`、`web`） |
| `client_version` | `String` | 否 | 客户端版本号 |

## 处理流程

1. 通过 `game_id` 查询推荐游戏记录
2. 获取或创建用户会话（`chat_sessions_user`）
3. 记录一条 `role=user` 消息，内容为游戏的 `reply` 字段
4. 记录一条 `role=assistant` 消息，内容为游戏的 `reply` 字段，`extensions` 包含一个 `game` 类型的 card

## 示例请求

```bash
BODY='{"user_id":"12345","game_id":"game_001","channel":"app","client_type":"android","client_version":"1.0.0"}'
SIGN=$(echo -n "${ASSISTANT_SECRET}/api/recommended-games/execute?body=${BODY}" | md5sum | awk '{print $1}')

curl -X POST "http://localhost:3300/api/recommended-games/execute?sign=${SIGN}" \
  -H "Content-Type: application/json; charset=UTF-8" \
  -d "${BODY}"
```

## 成功响应 `200 OK`

```json
{
  "code": 0,
  "data": {
    "id": 200,
    "session_id": 5,
    "user_id": 12345,
    "role": "assistant",
    "content": "推荐回复内容",
    "elapsed_ms": null,
    "extensions": [
      {
        "content_type": "card",
        "payload": {
          "type": "game",
          "info": {
            "id": "game_001",
            "name": "游戏名称",
            "channel": "app",
            "client_type": "android",
            "reason": "推荐理由",
            "game_tags": [{"name": "角色扮演", "type": 1}],
            "description": "推荐理由",
            "cover_image": "https://example.com/image.png",
            "computer_id": 10269,
            "platform_name": "Steam",
            "game_icon": "https://example.com/icon.png"
          }
        }
      }
    ],
    "created_at": "2026-06-10T14:30:00Z"
  },
  "message": "ok"
}
```

### `extensions[].payload.info` 字段说明

| 字段 | 类型 | 说明 |
|------|------|------|
| `id` | `String` | 游戏 ID（对应推荐游戏表的 `game_id`） |
| `name` | `String` | 游戏名称（对应推荐游戏表的 `game_name`） |
| `channel` | `String` | 请求时的渠道标识 |
| `client_type` | `String` | 请求时的客户端类型 |
| `reason` | `Option<String>` | 推荐理由 |
| `game_tags` | `Option<Value>` | 游戏标签数组，元素为 `{"name": "标签名", "type": 1}` |
| `description` | `Option<String>` | 游戏描述（同推荐理由） |
| `cover_image` | `Option<String>` | 游戏封面图片 URL |
| `computer_id` | `Option<i64>` | 外部游戏表关联的 computer_id |
| `platform_name` | `Option<String>` | 平台名称（如 Steam、PlayStation） |
| `game_icon` | `Option<String>` | 游戏图标 URL |

## 错误响应

| 状态码 | 说明 |
|--------|------|
| `400` | 参数校验失败（`user_id` 或 `game_id` 为空、MD5 签名校验失败） |
| `404` | `game_id` 对应的推荐游戏不存在 |
| `500` | 数据库查询或写入失败 |
